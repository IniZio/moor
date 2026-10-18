use std::sync::Arc;

use moor_core::seam::engine::EngineError;
use moor_core::{BoxId, StartReq};

use super::{req, world, World};
use crate::ops::start::run;
use crate::store::lease::{LeaseKind, LeaseRow, LeaseState};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// A start request for `name` with a goal.
fn goal_req(name: &str) -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req(name)
    }
}

/// A world whose Task `demo` has a ready Box with a known home and `engine` as its engine.
fn started(engine: &Arc<FakeEngine>) -> (World, BoxId) {
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let v = run(&w.cx, &req("demo")).unwrap();
    (w, BoxId::new(v.task.id))
}

/// The Task's leases on the launch directories.
fn dirs(w: &World, id: BoxId) -> Vec<LeaseRow> {
    let mut rows = w.cx.store().unwrap().leases(id).unwrap();
    rows.retain(|r| r.kind == LeaseKind::BoxDir && r.key.contains("/.moor-"));
    rows
}

/// Asserts the cred and tmp directories are leased in `state`.
fn assert_dirs(rows: &[LeaseRow], state: LeaseState) {
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r.state == state), "{rows:?}");
    for (row, name) in rows.iter().zip([".moor-cred-demo-", ".moor-tmp-demo-"]) {
        assert!(
            row.key.starts_with(&format!("/home/box/{name}")),
            "{}",
            row.key
        );
    }
}

#[test]
fn a_launch_holds_its_credential_and_tmp_directories() {
    let engine = Arc::new(FakeEngine::new().box_home("/home/box"));
    let (w, id) = started(&engine);
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_dirs(&dirs(&w, id), LeaseState::Held);
}

#[test]
fn a_failed_launch_leaves_its_directories_planned() {
    let engine = Arc::new(FakeEngine::new().box_home("/home/box").launch_result(Err(
        EngineError::Failed {
            step: "pane",
            message: "no tab".into(),
        },
    )));
    let (w, id) = started(&engine);
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_dirs(&dirs(&w, id), LeaseState::Planned);
}

#[test]
fn a_launch_whose_dirs_cannot_be_named_fails_before_launching() {
    let engine = Arc::new(
        FakeEngine::new()
            .box_home("/home/box")
            .fail_launch_dirs("no home"),
    );
    let (w, id) = started(&engine);
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    assert!(engine.launches().is_empty());
    assert!(dirs(&w, id).is_empty());
    let agent = v.agent;
    assert!(!agent.launched, "{agent:?}");
    let reason = format!("{:?}", agent.reason);
    assert!(reason.contains("no home"), "{reason}");
}
