//! Tests for how a setup re-run syncs a Box that already ran an agent.

use moor_core::domain::run::SyncMode;

use super::setup::{exit, failed_setup_with, is_setup_line, start_req, warning_codes};
use super::World;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// A failed first setup on a Task whose agent has run.
fn resumed_with_session() -> World {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, id) = failed_setup_with("setup: [a]\n", driver);
    w.cx.store()
        .unwrap()
        .begin_session(id, "agent-1", None)
        .unwrap();
    drop(warning_codes(&w));
    w
}

fn setup_syncs(w: &World) -> Vec<SyncMode> {
    w.driver
        .execs()
        .into_iter()
        .filter(|e| is_setup_line(&e.1.argv))
        .map(|e| e.1.sync)
        .collect()
}

#[test]
fn setup_re_run_after_an_agent_session_never_syncs_over_the_box() {
    let w = resumed_with_session();
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(setup_syncs(&w), [SyncMode::Full, SyncMode::None]);
    assert!(warning_codes(&w).contains(&"setup_rerun_unsynced".to_owned()));
}

#[test]
fn setup_re_run_before_any_agent_session_syncs_and_does_not_warn() {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, _) = failed_setup_with("setup: [a]\n", driver);
    drop(warning_codes(&w));
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(setup_syncs(&w), [SyncMode::Full, SyncMode::Full]);
    assert!(!warning_codes(&w).contains(&"setup_rerun_unsynced".to_owned()));
}
