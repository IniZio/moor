//! Tests for how a start on an existing Task re-syncs the Box checkout.

use moor_core::boxscript::boxwork::BOXWORK_NO_CHECKOUT;
use moor_core::domain::run::SyncMode;
use moor_core::seam::proc::Tool;
use moor_core::{BoxPath, ErrorKind, StartReq, TaskId};

use super::setup::{exit, failed_setup_with, start_req, warning_codes};
use super::{world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

/// A failed first setup on a Task whose agent has run.
fn resumed_with_session() -> World {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, id) = failed_setup_with("setup: [a]\n", driver);
    w.cx.store()
        .unwrap()
        .begin_session(id, "agent-1", None)
        .unwrap();
    drop(warning_codes(&w));
    w
}

/// The sync mode of every exec that only syncs the checkout.
fn checkout_syncs(w: &World) -> Vec<SyncMode> {
    w.driver
        .execs()
        .into_iter()
        .filter(|e| e.1.argv == ["git", "rev-parse", "--git-dir"])
        .map(|e| e.1.sync)
        .collect()
}

/// Answers the Box work probe: the host HEAD and the result file the Box holds.
fn probe_says(w: &World, result: &str) {
    let head = "0123456789abcdef0123456789abcdef01234567\n";
    w.proc.before(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(head.to_owned()),
    );
    let path = BoxPath::under("/work/cbx_1/demo", ".moor/boxwork").unwrap();
    w.driver.set_peek_text("box-1", &path, result);
}

/// A started Task whose agent session is `live` or has ended, ready to be started again with no
/// setup to re-run.
fn attached(driver: FakeDriver, live: bool) -> World {
    let w = world(driver);
    let first = run(&w.cx, &start_req()).unwrap();
    let store = w.cx.store().unwrap();
    let session = store
        .begin_session(TaskId::new(first.task.id), "agent-1", None)
        .unwrap();
    if !live {
        store.end_session(session, Some(0), None).unwrap();
    }
    drop(warning_codes(&w));
    w
}

/// A started Task whose agent session has ended.
fn attached_with_session(driver: FakeDriver) -> World {
    attached(driver, false)
}

#[test]
fn setup_re_run_over_unsaved_box_work_never_syncs_over_the_box() {
    let w = resumed_with_session();
    probe_says(&w, "ahead=1 dirty=0");
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(checkout_syncs(&w), [SyncMode::Full]);
    assert!(warning_codes(&w).contains(&"resync_skipped".to_owned()));
}

#[test]
fn setup_re_run_syncs_a_box_with_no_unsaved_work() {
    let w = resumed_with_session();
    probe_says(&w, "ahead=0 dirty=0");
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
    assert!(!warning_codes(&w).contains(&"resync_skipped".to_owned()));
}

#[test]
fn setup_re_run_syncs_a_box_whose_first_sync_left_no_checkout() {
    let driver =
        FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1)), Ok(exit(BOXWORK_NO_CHECKOUT))]);
    let (w, id) = failed_setup_with("setup: [a]\n", driver);
    w.cx.store()
        .unwrap()
        .begin_session(id, "agent-1", None)
        .unwrap();
    probe_says(&w, "");
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
}

#[test]
fn setup_re_run_before_any_agent_session_syncs_and_does_not_warn() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, _) = failed_setup_with("setup: [a]\n", driver);
    drop(warning_codes(&w));
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
    assert!(!warning_codes(&w).contains(&"resync_skipped".to_owned()));
}

#[test]
fn attach_to_a_task_with_no_session_syncs_the_checkout_alone() {
    let w = world(FakeDriver::new());
    run(&w.cx, &start_req()).unwrap();
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
}

#[test]
fn attach_syncs_a_box_with_no_unsaved_work() {
    let w = attached_with_session(FakeDriver::new());
    probe_says(&w, "ahead=0 dirty=0");
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
}

#[test]
fn attach_syncs_a_box_with_no_checkout() {
    let w = attached_with_session(
        FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(BOXWORK_NO_CHECKOUT))]),
    );
    probe_says(&w, "");
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
}

#[test]
fn attach_keeps_unsaved_box_work_and_warns() {
    let w = attached_with_session(FakeDriver::new());
    probe_says(&w, "ahead=0 dirty=2");
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full]);
    assert!(warning_codes(&w).contains(&"resync_skipped".to_owned()));
}

#[test]
fn attach_to_a_live_agent_never_syncs_and_warns() {
    let w = attached(FakeDriver::new(), true);
    probe_says(&w, "ahead=0 dirty=0");
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full]);
    assert!(warning_codes(&w).contains(&"resync_skipped".to_owned()));
}

#[test]
fn attach_warns_when_the_re_sync_fails_and_no_agent_is_live() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let w = attached_with_session(driver);
    probe_says(&w, "ahead=0 dirty=0");
    run(&w.cx, &start_req()).unwrap();
    assert!(warning_codes(&w).contains(&"resync_failed".to_owned()));
}

#[test]
fn start_that_launches_the_agent_aborts_when_the_re_sync_fails() {
    let w = world(FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]));
    let first = run(&w.cx, &start_req()).unwrap();
    let mut again = start_req();
    again.goal = Some("fix it".to_owned());
    let err = run(&w.cx, &again).unwrap_err();
    assert!(err
        .error
        .to_string()
        .contains("syncing the checkout into the Box failed"));
    let id = TaskId::new(first.task.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    assert!(events.iter().any(|e| e.kind == "setup_failed"));
}

/// A failed first setup whose re-run sync fails.
fn failing_re_run_sync() -> World {
    let driver =
        FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1)), Ok(exit(1)), Ok(exit(0))]);
    let (w, _) = failed_setup_with("setup: [a]\n", driver);
    drop(warning_codes(&w));
    w
}

#[test]
fn setup_re_run_with_no_agent_launch_warns_when_the_sync_fails_and_attaches() {
    let w = failing_re_run_sync();
    let req = StartReq {
        no_agent: true,
        ..start_req()
    };
    let again = run(&w.cx, &req).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert!(warning_codes(&w).contains(&"resync_failed".to_owned()));
}

#[test]
fn setup_re_run_that_launches_the_agent_aborts_when_the_sync_fails() {
    let w = failing_re_run_sync();
    let req = StartReq {
        goal: Some("ship it".into()),
        ..start_req()
    };
    let err = run(&w.cx, &req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(!warning_codes(&w).contains(&"resync_failed".to_owned()));
}
