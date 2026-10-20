//! Tests for auto-sleep.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, SleepOutcome};
use moor_core::secret::Secret;
use moor_core::{BoxId, BoxName, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::view::box_view;
use crate::store::{
    Fenced, NewRun, ProxyPorts, ReadyProc, Reserve, TunnelAck, TunnelRequest, TunnelWant,
};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::rng::FakeRng;
use crate::testing::surface::FakeSurface;

/// Time enough to pass the default `box.sleep_after`.
const LONG: Duration = Duration::from_mins(31);

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
        Path::new("/home/me/proj"),
        driver.clone(),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set("/home/me/wt/t".to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
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
    run(&f.cx, f.cx.store().unwrap(), f.id);
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

/// Make the Box's Proxy supervisor look live: ready, leased and just beating.
fn live_proxy(f: &Fx) {
    let store = f.cx.store().unwrap();
    let lease = Duration::from_secs(30);
    let got = store
        .reserve(
            f.id,
            &Secret::new(b"n".to_vec()),
            &FakeRng::default(),
            lease,
        )
        .unwrap();
    assert!(matches!(got, Reserve::Reserved(_)), "{got:?}");
    store.claim_lease(f.id, "A", lease).unwrap();
    let box_port = store.proxy(f.id).unwrap().unwrap().box_port;
    let ports = ProxyPorts {
        box_port,
        host_port: 41_234,
    };
    let proc = ReadyProc {
        pid: 4242,
        handle: "tunnel-1",
        boot_id: None,
    };
    let ready = store.mark_ready(f.id, "A", ports, proc).unwrap();
    assert_eq!(ready, Fenced::Applied);
}

/// The stored tunnel request for the Box.
fn tunnel_request(f: &Fx) -> Option<TunnelRequest> {
    f.cx.store().unwrap().tunnel_request(f.id).unwrap()
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
    pass(&f, Duration::from_mins(29));
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
    f.cx.store()
        .unwrap()
        .begin_run(NewRun {
            box_id: f.id,
            argv: vec!["make".into()],
            record: None,
            sha: None,
            sync: "none".into(),
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
fn auto_sleep_parks_a_live_proxy_tunnel_first() {
    let f = idle();
    pass(&f, LONG);
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    store
        .ack_tunnel(f.id, &TunnelAck::Parked { seq: 1 })
        .unwrap();
    auto_sleep(&f);
    let r = tunnel_request(&f).unwrap();
    assert_eq!((r.want, r.seq), (TunnelWant::Parked, 1));
    assert_eq!(
        states(&f),
        (BoxLifecycle::Asleep, SessionState::Idle, BoxState::Asleep)
    );
}

#[test]
fn auto_sleep_of_a_box_that_cannot_sleep_asks_for_a_clearing_restore() {
    let f = refusing();
    pass(&f, LONG);
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    store
        .ack_tunnel(f.id, &TunnelAck::Parked { seq: 1 })
        .unwrap();
    auto_sleep(&f);
    let r = tunnel_request(&f).unwrap();
    assert_eq!((r.want, r.seq), (TunnelWant::UpAwake, 2));
}

#[test]
fn auto_sleep_sweeps_stale_credential_dirs_before_the_box_sleeps() {
    let f = idle();
    pass(&f, LONG);
    auto_sleep(&f);
    let calls = f.driver.calls();
    let at_sweep = calls.iter().position(|c| c.starts_with("sweep ")).unwrap();
    let at_nap = calls.iter().position(|c| c.starts_with("sleep ")).unwrap();
    assert!(at_sweep < at_nap, "{calls:?}");
    assert_eq!(f.driver.sweeps().len(), 1);
}

#[test]
fn auto_sleep_tries_a_timed_out_sweep_once_warns_and_still_sleeps() {
    let driver = FakeDriver::new().fail_sweep(DriverError::Timeout(Duration::from_secs(30)));
    let f = idle_with(driver);
    pass(&f, LONG);
    auto_sleep(&f);
    assert_eq!(
        states(&f),
        (BoxLifecycle::Asleep, SessionState::Idle, BoxState::Asleep)
    );
    assert_eq!(sleep_calls(&f), 1);
    assert_eq!(
        f.driver.sweeps().len(),
        1,
        "a settle tick never retries inline"
    );
    let codes: Vec<_> = f.cx.take_warnings().into_iter().map(|w| w.code).collect();
    assert_eq!(codes, ["cred_sweep_failed"]);
}
