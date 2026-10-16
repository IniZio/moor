//! Tests for how a setup re-run syncs a Box that already ran an agent.

use moor_core::domain::run::SyncMode;
use moor_core::{ErrorKind, StartReq};

use super::setup::{exit, failed_setup_with, start_req, warning_codes};
use super::World;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

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

#[test]
fn setup_re_run_after_an_agent_session_never_syncs_over_the_box() {
    let w = resumed_with_session();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(checkout_syncs(&w), [SyncMode::Full]);
    assert!(warning_codes(&w).contains(&"setup_rerun_unsynced".to_owned()));
}

#[test]
fn setup_re_run_before_any_agent_session_syncs_and_does_not_warn() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, _) = failed_setup_with("setup: [a]\n", driver);
    drop(warning_codes(&w));
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(checkout_syncs(&w), [SyncMode::Full, SyncMode::Full]);
    assert!(!warning_codes(&w).contains(&"setup_rerun_unsynced".to_owned()));
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
