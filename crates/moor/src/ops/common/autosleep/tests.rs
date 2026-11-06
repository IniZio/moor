//! Tests for auto-sleep.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, SleepOutcome};
use moor_core::{BoxId, BoxName, ProviderId};

use super::{run, AgentRead};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::view::box_view;
use crate::store::NewRun;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};
use crate::testing::surface::FakeSurface;

/// Time enough to pass the default `box.sleep_after`.
const LONG: Duration = Duration::from_mins(6);

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: BoxId,
}

/// Box `t` Working with a live machine.
fn working_with(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    Fx {
        _dir: dir,
        cx,
        driver,
        id,
    }
}

/// A fixture whose Box became Idle just now.
fn idle_with(driver: FakeDriver) -> Fx {
    let f = working_with(driver);
    let d = Decision::new(f.id).session(SessionInput::AgentQuiet);
    f.cx.store().unwrap().apply(d).unwrap();
    f
}

fn idle() -> Fx {
    idle_with(FakeDriver::new())
}

/// Let `d` pass on the fake clock.
fn pass(f: &Fx, d: Duration) {
    f.cx.clock().sleep(d);
}

fn auto_sleep(f: &Fx) {
    run(&f.cx, f.cx.store().unwrap(), f.id, AgentRead::Plain);
}

fn auto_sleep_blocked(f: &Fx) {
    run(&f.cx, f.cx.store().unwrap(), f.id, AgentRead::Blocked);
}

/// Kinds of the events recorded for the Box.
fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

/// Box lifecycle, Session and Box states now.
fn states(f: &Fx) -> (BoxLifecycle, SessionState, BoxState) {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    (row.lifecycle, row.session, row.boxed.unwrap().state)
}

fn sleep_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("sleep "))
        .count()
}

#[test]
fn a_box_idle_long_enough_sleeps() {
    let f = idle();
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Asleep, SessionState::Idle, BoxState::Asleep)
    );
    let k = kinds(&f);
    assert!(k.contains(&"asleep".to_owned()), "{k:?}");
    assert!(k.contains(&"box_asleep".to_owned()), "{k:?}");
    assert_eq!(sleep_calls(&f), 1);
}

#[test]
fn a_native_sleep_counts_as_slept() {
    let f = idle_with(FakeDriver::new().sleep_outcome(SleepOutcome::Native));
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Asleep, SessionState::Idle, BoxState::Asleep)
    );
}

#[test]
fn a_box_not_idle_long_enough_is_left_alone() {
    let f = idle();
    pass(&f, Duration::from_mins(4));
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Ready, SessionState::Idle, BoxState::Ready)
    );
    assert_eq!(sleep_calls(&f), 0);
}

#[test]
fn a_working_box_never_sleeps() {
    let f = working_with(FakeDriver::new());
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Ready, SessionState::Working, BoxState::Ready)
    );
    assert_eq!(sleep_calls(&f), 0);
}

#[test]
fn a_held_box_lock_skips_the_sleep_silently() {
    let f = idle();
    pass(&f, LONG);
    let _held = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Ready, SessionState::Idle, BoxState::Ready)
    );
    assert_eq!(sleep_calls(&f), 0);
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn a_run_in_flight_skips_the_sleep() {
    let f = idle();
    let _claim =
        f.cx.store()
            .unwrap()
            .begin_run(NewRun {
                box_id: f.id,
                argv: vec!["make".into()],
                record: None,
                sha: None,
                pid: 4242,
            })
            .unwrap();
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(states(&f).2, BoxState::Ready);
    assert_eq!(sleep_calls(&f), 0);
}

#[test]
fn a_box_that_cannot_sleep_stays_up_and_says_why_once() {
    let f = idle_with(
        FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep("provider cannot wake".into())),
    );
    pass(&f, LONG);
    auto_sleep(&f);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Ready, SessionState::Idle, BoxState::Ready)
    );
    assert_eq!(sleep_calls(&f), 1);
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    let shown = box_view(&row, f.cx.config()).driver.unwrap().sleep.reason;
    assert_eq!(shown.as_deref(), Some("provider cannot wake"));
    let stays = kinds(&f).iter().filter(|k| *k == "stays_up").count();
    assert_eq!(stays, 1);
}

/// The stored sleep reason of the Box.
fn reason(f: &Fx) -> Option<String> {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    row.boxed.unwrap().sleep_reason
}

fn step(f: &Fx, input: SessionInput) {
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).session(input))
        .unwrap();
}

fn refusing() -> Fx {
    idle_with(
        FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep("provider cannot wake".into())),
    )
}

#[test]
fn a_known_refusal_is_not_asked_again_while_idle() {
    let f = refusing();
    pass(&f, LONG);
    auto_sleep(&f);
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(sleep_calls(&f), 1);
    assert_eq!(reason(&f).as_deref(), Some("provider cannot wake"));
}

#[test]
fn leaving_idle_clears_the_refusal() {
    let f = refusing();
    pass(&f, LONG);
    auto_sleep(&f);
    step(&f, SessionInput::AgentBusy);
    assert_eq!(states(&f).1, SessionState::Working);
    assert_eq!(reason(&f), None);
}

#[test]
fn the_next_idle_stretch_asks_again() {
    let f = refusing();
    pass(&f, LONG);
    auto_sleep(&f);
    step(&f, SessionInput::AgentBusy);
    step(&f, SessionInput::AgentQuiet);
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(sleep_calls(&f), 2);
    assert_eq!(reason(&f).as_deref(), Some("provider cannot wake"));
}

#[test]
fn an_explicit_sleep_still_asks_the_driver() {
    let f = refusing();
    pass(&f, LONG);
    auto_sleep(&f);
    let req = moor_core::BoxReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
    };
    let err = crate::ops::sleep::run(&f.cx, &req);
    assert!(err.is_err());
    assert_eq!(sleep_calls(&f), 2);
}

#[test]
fn an_unknown_provider_stays_up_and_says_why() {
    let reason = "provider nope is unknown; box stays up";
    let f = idle_with(FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep(reason.into())));
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Ready, SessionState::Idle, BoxState::Ready)
    );
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    let shown = box_view(&row, f.cx.config()).driver.unwrap().sleep.reason;
    assert_eq!(shown.as_deref(), Some(reason));
}

#[test]
fn a_driver_failure_warns_and_changes_nothing() {
    let f = idle_with(FakeDriver::new().fail_sleep(DriverError::Failed {
        summary: "boom".into(),
    }));
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Ready, SessionState::Idle, BoxState::Ready)
    );
    let warned = f.cx.take_warnings();
    assert_eq!(warned.len(), 1);
    assert_eq!(warned.first().unwrap().code, "auto_sleep_failed");
}

#[test]
fn a_box_waiting_on_a_prompt_sleeps_after_five_minutes() {
    let f = idle();
    pass(&f, Duration::from_mins(4));
    auto_sleep_blocked(&f);
    assert_eq!(sleep_calls(&f), 0);
    pass(&f, Duration::from_mins(1));
    auto_sleep_blocked(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Asleep, SessionState::Idle, BoxState::Asleep)
    );
}

#[test]
fn a_working_box_sleeps_after_two_hours_awake() {
    let f = working_with(FakeDriver::new());
    pass(&f, Duration::from_mins(119));
    auto_sleep(&f);
    assert_eq!(sleep_calls(&f), 0);
    pass(&f, Duration::from_mins(1));
    auto_sleep(&f);
    assert_eq!(
        states(&f).0,
        BoxLifecycle::Asleep,
        "a Box awake two hours sleeps while working"
    );
}

#[test]
fn a_wake_restarts_the_awake_clock() {
    let f = working_with(FakeDriver::new());
    pass(&f, Duration::from_mins(119));
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::Sleep))
        .unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::Wake))
        .unwrap();
    pass(&f, Duration::from_mins(119));
    auto_sleep(&f);
    assert_eq!(sleep_calls(&f), 0);
}

#[test]
fn auto_sleep_parks_the_tunnel_first() {
    let mut f = idle();
    let host = FakeProxyHost::new();
    f.cx = f.cx.with_proxy_host(Arc::new(host.clone()));
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(host.calls(), vec![ProxyHostCall::Park(f.id)]);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Asleep, SessionState::Idle, BoxState::Asleep)
    );
}

#[test]
fn auto_sleep_of_a_box_that_cannot_sleep_restores_its_tunnel() {
    let mut f = refusing();
    let host = FakeProxyHost::new();
    f.cx = f.cx.with_proxy_host(Arc::new(host.clone()));
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        host.calls(),
        vec![ProxyHostCall::Park(f.id), ProxyHostCall::Restore(f.id)]
    );
}
