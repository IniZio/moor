//! Tests for `sleep`.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::driver::{AcquireReq, Driver, SleepKind, SleepOutcome};
use moor_core::seam::surface::SurfaceError;
use moor_core::{BoxId, BoxName, BoxReq, ErrorKind, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::view::box_view;
use crate::store::{Fenced, NewRun, ReadyProc, Reserve, TunnelAck, TunnelRequest, TunnelWant};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::rng::FakeRng;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    id: BoxId,
}

/// Box `t` with a live machine, so the Box is Working.
fn fx_with(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let surface = Arc::new(FakeSurface::new());
    let cx = Cx::fake(
        dir.path(),
        driver.clone(),
        surface.clone(),
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
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
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
        surface,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new())
}

/// A fixture whose Box is Idle.
fn idle() -> Fx {
    idle_with(FakeDriver::new())
}

fn idle_with(driver: FakeDriver) -> Fx {
    let f = fx_with(driver);
    let d = Decision::new(f.id).session(SessionInput::AgentQuiet);
    f.cx.store().unwrap().apply(d).unwrap();
    f
}

fn req() -> BoxReq {
    BoxReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
    }
}

/// Kinds of the events recorded for the Box.
fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

/// Box and machine states now.
fn states(f: &Fx) -> (BoxLifecycle, BoxState) {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    (row.lifecycle, row.boxed.unwrap().state)
}

fn session(f: &Fx) -> SessionState {
    f.cx.store()
        .unwrap()
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .session
}

/// The stored sleep reason of the Box.
fn reason(f: &Fx) -> Option<String> {
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    row.boxed.unwrap().sleep_reason
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
    let got = store.reserve(f.id, &FakeRng::default(), lease).unwrap();
    assert!(matches!(got, Reserve::Reserved(_)), "{got:?}");
    store.claim_lease(f.id, "A", lease).unwrap();
    let box_port = store.proxy(f.id).unwrap().unwrap().box_port;
    let proc = ReadyProc {
        pid: 4242,
        handle: "tunnel-1",
        boot_id: None,
    };
    let ready = store.mark_ready(f.id, "A", box_port, proc).unwrap();
    assert_eq!(ready, Fenced::Applied);
}

/// The stored tunnel request for the Box.
fn tunnel_request(f: &Fx) -> Option<TunnelRequest> {
    f.cx.store().unwrap().tunnel_request(f.id).unwrap()
}

/// Begin a run owned by a live pid.
fn begin_run(f: &Fx) {
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
}

#[test]
fn sleeps_an_idle_box() {
    let f = idle();
    let v = run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Asleep, BoxState::Asleep));
    let k = kinds(&f);
    assert!(k.contains(&"asleep".to_owned()), "{k:?}");
    assert!(k.contains(&"box_asleep".to_owned()), "{k:?}");
    assert_eq!(v.box_name, "t");
    assert!(!v.woke);
    assert_eq!(v.driver.unwrap().state, BoxState::Asleep);
    assert_eq!(sleep_calls(&f), 1);
}

#[test]
fn sleeps_a_working_box_without_a_run() {
    let f = fx();
    assert_eq!(states(&f).0, BoxLifecycle::Ready);
    assert_eq!(session(&f), SessionState::Working);
    run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Asleep, BoxState::Asleep));
}

#[test]
fn refuses_while_a_run_is_in_flight() {
    let f = fx();
    begin_run(&f);
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("run is in flight"),
        "{}",
        err.error
    );
    assert_eq!(sleep_calls(&f), 0);
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Ready));
    assert_eq!(session(&f), SessionState::Working);
}

#[test]
fn native_sleep_transitions_like_slept() {
    let f = idle_with(FakeDriver::new().sleep_outcome(SleepOutcome::Native));
    run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Asleep, BoxState::Asleep));
}

#[test]
fn cannot_sleep_refuses_and_stores_the_reason() {
    let f = idle_with(
        FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep("provider cannot wake".into())),
    );
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("provider cannot wake"),
        "{}",
        err.error
    );
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Ready));
    assert_eq!(session(&f), SessionState::Idle);
    assert_eq!(reason(&f).as_deref(), Some("provider cannot wake"));
    let row = f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    let shown = box_view(&row, f.cx.config()).driver.unwrap().sleep.reason;
    assert_eq!(shown.as_deref(), Some("provider cannot wake"));
    assert!(!kinds(&f).contains(&"asleep".to_owned()));
}

#[test]
fn unknown_provider_refuses_naming_the_provider() {
    let why = "provider nope is unknown; box stays up";
    let f = idle_with(FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep(why.into())));
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains(why), "{}", err.error);
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Ready));
    assert_eq!(session(&f), SessionState::Idle);
    assert_eq!(reason(&f).as_deref(), Some(why));
}

#[test]
fn a_later_successful_sleep_clears_the_reason() {
    let f = idle();
    let d = Decision::new(f.id).set(Fields {
        box_sleep_reason: Field::Set("old".to_owned()),
        ..Fields::default()
    });
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(reason(&f).as_deref(), Some("old"));
    run(&f.cx, &req()).unwrap();
    assert_eq!(reason(&f), None);
}

#[test]
fn unknown_box_fails() {
    let f = fx();
    let other = BoxReq {
        box_name: BoxName::parse_allow_bare("nope").unwrap(),
    };
    let err = run(&f.cx, &other).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(
        err.error.to_string().contains("no open Box nope"),
        "{}",
        err.error
    );
}

#[test]
fn refuses_when_the_box_lock_is_held() {
    let f = idle();
    let _held = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("busy"), "{}", err.error);
    assert_eq!(sleep_calls(&f), 0);
}

#[test]
fn an_asleep_box_is_left_alone() {
    let f = idle();
    run(&f.cx, &req()).unwrap();
    let v = run(&f.cx, &req()).unwrap();
    assert!(!v.woke);
    assert_eq!(sleep_calls(&f), 1);
    assert_eq!(states(&f), (BoxLifecycle::Asleep, BoxState::Asleep));
}

/// A Box and machine seeded through `Requested`: Box `SettingUp`, machine Starting.
fn starting() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let surface = Arc::new(FakeSurface::new());
    let cx = Cx::fake(
        dir.path(),
        driver.clone(),
        surface.clone(),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    Fx {
        _dir: dir,
        cx,
        driver,
        surface,
        id,
    }
}

/// Fields that give the Box a handle, so only its state can make `sleep` refuse.
fn with_handle() -> Fields {
    Fields {
        box_handle: Field::Set(moor_core::name::BoxHandle::new("box-1")),
        ..Fields::default()
    }
}

/// Assert `run` refuses without a Driver sleep call and leaves the states alone.
fn assert_refused_untouched(f: &Fx) {
    let before = states(f);
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused, "{}", err.error);
    assert_eq!(sleep_calls(f), 0);
    assert_eq!(states(f), before);
}

#[test]
fn refuses_a_box_that_is_still_setting_up() {
    let f = starting();
    let d = Decision::new(f.id).box_input(BoxInput::Acquired {
        handle: moor_core::name::BoxHandle::new("box-1"),
    });
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::SettingUp, BoxState::Ready));
    assert_refused_untouched(&f);
}

#[test]
fn refuses_a_working_box_whose_box_is_starting() {
    let f = starting();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .set(with_handle());
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Starting));
    assert_eq!(session(&f), SessionState::Working);
    assert_refused_untouched(&f);
}

#[test]
fn refuses_a_working_box_whose_box_failed() {
    let f = starting();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::AcquireFailed {
            cause: "no capacity".to_owned(),
            attempted: None,
        })
        .set(with_handle());
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Failed));
    assert_eq!(session(&f), SessionState::Working);
    assert_refused_untouched(&f);
}

#[test]
fn refuses_an_idle_box_whose_ready_box_has_no_handle() {
    let f = idle();
    let d = Decision::new(f.id).set(Fields {
        box_handle: Field::Clear,
        ..Fields::default()
    });
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Ready));
    assert_eq!(session(&f), SessionState::Idle);
    assert_refused_untouched(&f);
}

#[test]
fn sleep_label_follows_provider_sleep_support() {
    for (kind, after) in [(SleepKind::None, None), (SleepKind::Suspend, Some("30m"))] {
        let f = idle_with(FakeDriver::new().sleep_kind(kind));
        let v = run(&f.cx, &req()).unwrap();
        assert_eq!(v.driver.unwrap().sleep.after.as_deref(), after);
    }
}

#[test]
fn sleep_parks_a_live_proxy_tunnel_first() {
    let f = idle();
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    store
        .ack_tunnel(f.id, &TunnelAck::Parked { seq: 1 })
        .unwrap();
    run(&f.cx, &req()).unwrap();
    let r = tunnel_request(&f).unwrap();
    assert_eq!((r.want, r.seq), (TunnelWant::Parked, 1));
    assert_eq!(states(&f), (BoxLifecycle::Asleep, BoxState::Asleep));
}

#[test]
fn sleep_leaves_the_box_awake_when_the_tunnel_never_parks() {
    let f = idle();
    live_proxy(&f);
    assert!(run(&f.cx, &req()).is_err());
    assert_eq!(sleep_calls(&f), 0);
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Ready));
    assert_eq!(session(&f), SessionState::Idle);
}

#[test]
fn a_box_that_cannot_sleep_gets_its_tunnel_back() {
    let f = idle_with(FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep("no".into())));
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    store
        .ack_tunnel(f.id, &TunnelAck::Parked { seq: 1 })
        .unwrap();
    assert!(run(&f.cx, &req()).is_err());
    let r = tunnel_request(&f).unwrap();
    assert_eq!((r.want, r.seq), (TunnelWant::UpAwake, 2));
}

/// Give the Box a live agent session in pane `p-1`.
fn with_agent_pane(f: &Fx) {
    f.surface.have_pane("p-1");
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "agent-1", Some("p-1"))
        .unwrap();
}

#[test]
fn sleep_closes_the_agent_pane() {
    let f = idle();
    with_agent_pane(&f);
    run(&f.cx, &req()).unwrap();
    assert!(f.surface.calls().contains(&"close_pane p-1".to_owned()));
    assert_eq!(sleep_calls(&f), 1);
}

#[test]
fn sleep_closes_the_pane_before_pausing_the_box() {
    let f = idle();
    with_agent_pane(&f);
    f.surface
        .set_fail_close_pane(SurfaceError::Unavailable("boom".into()));
    let err = run(&f.cx, &req()).unwrap_err();
    assert!(err.error.to_string().contains("agent pane"), "{err:?}");
    assert_eq!(sleep_calls(&f), 0);
    assert_eq!(states(&f), (BoxLifecycle::Ready, BoxState::Ready));
    assert_eq!(session(&f), SessionState::Idle);
}

#[test]
fn sleep_with_the_pane_already_gone_still_sleeps() {
    let f = idle();
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "agent-1", Some("p-1"))
        .unwrap();
    run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (BoxLifecycle::Asleep, BoxState::Asleep));
}

#[test]
fn sleep_makes_no_box_call_but_the_sleep() {
    let f = idle();
    let before = f.driver.calls();
    run(&f.cx, &req()).unwrap();
    let calls = f.driver.calls();
    assert_eq!(
        calls.get(before.len()..),
        Some(&["sleep box-1".to_owned()][..])
    );
}
