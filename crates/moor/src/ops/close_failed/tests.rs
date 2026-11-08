//! Tests for `close-failed`.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::request::CloseFailedReq;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::user::{Identity, Viewer};
use moor_core::{BoxId, BoxName, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::testing::cx::new_box;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

struct World {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
}

fn world(driver: FakeDriver) -> World {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(dir.path(), driver.clone(), Arc::new(ScriptedProc::new()));
    World {
        _dir: dir,
        cx,
        driver,
    }
}

fn name(n: &str) -> BoxName {
    BoxName::parse_allow_bare(n).unwrap()
}

/// Inserts Box `n` and admits it; with `machine` it also holds an acquired machine.
fn admitted(w: &World, n: &str, machine: bool) -> BoxId {
    let store = w.cx.store().unwrap();
    let id = store.apply(new_box(name(n))).unwrap().box_id;
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(Fields {
                    box_provider: Field::Set(ProviderId::new("p")),
                    workspace_id: Field::Set("ws-1".into()),
                    box_workdir: Field::Set("/work/box-1".into()),
                    ..Fields::default()
                }),
        )
        .unwrap();
    if machine {
        let got = w
            .driver
            .acquire(&AcquireReq {
                pool_machine: None,
                provider: ProviderId::new("p"),
                box_id: id,
                name: name(n),
                class: None,
                nonce: "n".to_owned(),
                cache: moor_core::cache::CachePlan::none(),
            })
            .unwrap();
        store
            .apply(Decision::new(id).box_input(BoxInput::Acquired { handle: got.handle }))
            .unwrap();
    }
    id
}

fn setup_failed(w: &World, n: &str, machine: bool) -> BoxId {
    let id = admitted(w, n, machine);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::SetupFailed {
            cause: "clone failed".to_owned(),
            which: None,
        }))
        .unwrap();
    id
}

fn ready(w: &World, n: &str) -> BoxId {
    let id = admitted(w, n, false);
    w.cx.store()
        .unwrap()
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: SetupOutcome::Ok,
                })
                .session(SessionInput::Started),
        )
        .unwrap();
    id
}

fn lifecycle(w: &World, id: BoxId) -> BoxLifecycle {
    w.cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .lifecycle
}

fn closed_payload(w: &World, id: BoxId) -> serde_json::Value {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    events
        .iter()
        .find(|e| e.kind == "closed")
        .expect("a closed event")
        .payload
        .clone()
}

fn req(force: bool) -> CloseFailedReq {
    CloseFailedReq { force }
}

#[test]
fn closes_every_setup_failed_box_and_leaves_the_rest() {
    let w = world(FakeDriver::new());
    let first = setup_failed(&w, "a", false);
    let second = setup_failed(&w, "b", true);
    let healthy = ready(&w, "c");
    let v = run(&w.cx, &req(false)).unwrap();
    assert_eq!(v.closed, [name("a"), name("b")]);
    assert!(v.failed.is_empty(), "{:?}", v.failed);
    assert_eq!(lifecycle(&w, first), BoxLifecycle::Closed);
    assert_eq!(lifecycle(&w, second), BoxLifecycle::Closed);
    assert_eq!(lifecycle(&w, healthy), BoxLifecycle::Ready);
    for id in [first, second] {
        assert_eq!(
            closed_payload(&w, id),
            serde_json::json!({"from": "setup_failed", "reason": "close_failed"})
        );
    }
}

#[test]
fn another_users_box_is_not_closed_for_a_non_operator() {
    let mut w = world(FakeDriver::new());
    let theirs = setup_failed(&w, "theirs", false);
    let (other, _) =
        w.cx.store()
            .unwrap()
            .enroll(&Identity::local(2000), None)
            .unwrap();
    w.cx = w.cx.with_viewer(Viewer::User(other));
    let v = run(&w.cx, &req(false)).unwrap();
    assert!(v.closed.is_empty() && v.failed.is_empty(), "{v:?}");
    assert_eq!(lifecycle(&w, theirs), BoxLifecycle::SetupFailed);
}

#[test]
fn a_box_that_will_not_close_lands_in_failed_and_the_rest_still_close() {
    let w = world(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: "stuck".into(),
    }));
    let stuck = setup_failed(&w, "stuck", true);
    let fine = setup_failed(&w, "fine", false);
    let v = run(&w.cx, &req(false)).unwrap();
    assert_eq!(v.closed, [name("fine")]);
    let only = v.failed.first().expect("one failure");
    assert_eq!(v.failed.len(), 1);
    assert_eq!(only.r#box, name("stuck"));
    assert!(only.error.contains("stuck"), "{only:?}");
    assert_eq!(lifecycle(&w, stuck), BoxLifecycle::SetupFailed);
    assert_eq!(lifecycle(&w, fine), BoxLifecycle::Closed);
}
