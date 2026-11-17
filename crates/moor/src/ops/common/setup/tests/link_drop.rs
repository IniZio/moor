//! A machine's link that drops while a setup line runs: setup waits for the relink and follows
//! the same run, and a failure names the line that was running.

use std::sync::{Arc, Mutex};

use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{DriverError, ExecOutcome};

use super::super::{run_setup as run_setup_with, SetupRun};
use super::{cfg, rig_on, RUN};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// What the machine adapter says when the stream it carried a command on ends early.
fn cut() -> Result<ExecOutcome, DriverError> {
    Err(DriverError::Transport {
        stderr_tail: "the stream ended before the command did".to_owned(),
    })
}

/// What the machine adapter says while the controller holds no link to the machine.
fn unlinked() -> Result<ExecOutcome, DriverError> {
    Err(DriverError::Unavailable(
        "machine 1 is not linked".to_owned(),
    ))
}

/// The log the first attach reads: line `a` passed and line `b` began.
const B_RUNNING: &str = "moor setup: $ a\nmoor setup: $ b\n";

/// Runs setup lines `a`, `b`, `c` against `driver`; the outcome, the events and the driver.
fn run(driver: FakeDriver) -> (SetupRun, Vec<ExtraEvent>, Vec<String>, Vec<Vec<String>>) {
    let d = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let r = rig_on(d.path(), driver.fail_copy_out(DriverError::NotFound), clock);
    let seen = Mutex::new(Vec::new());
    let got = run_setup_with(
        &r.cx,
        &r.handle,
        ("p", RUN),
        &r.workdir,
        &d.path().join("logs"),
        &cfg(&["a", "b", "c"], &[]),
        None,
        &|e| seen.lock().unwrap().push(e),
    )
    .unwrap();
    let argvs = r
        .driver
        .execs()
        .into_iter()
        .map(|(_, s, _)| s.argv)
        .collect();
    (got, seen.into_inner().unwrap(), r.driver.calls(), argvs)
}

fn reattached(events: &[ExtraEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            ExtraEvent::SetupReattached { index, .. } => Some(*index),
            _ => None,
        })
        .collect()
}

#[test]
fn a_link_lost_mid_line_waits_for_the_relink_and_follows_the_same_run_to_its_exit() {
    let driver = FakeDriver::new().exec_stdout([B_RUNNING]).exec_results(
        std::iter::once(cut())
            .chain((0..30).map(|_| unlinked()))
            .chain([Ok(ExecOutcome::Exited { code: 0 })]),
    );
    let (got, events, calls, argvs) = run(driver);
    assert!(matches!(got, SetupRun::Ok { .. }), "{got:?}");
    assert_eq!(reattached(&events), [2], "{events:?}");
    assert!(
        argvs.iter().all(|a| Some(a) == argvs.first()),
        "every attach follows the same run"
    );
    assert!(!calls.iter().any(|c| c.starts_with("cancel_run")));
}

#[test]
fn a_nonzero_exit_after_the_relink_still_fails_and_names_its_line() {
    let driver = FakeDriver::new()
        .exec_stdout([B_RUNNING, "", "", B_RUNNING])
        .exec_results([
            cut(),
            unlinked(),
            unlinked(),
            Ok(ExecOutcome::Exited { code: 1 }),
        ]);
    let (got, events, _, _) = run(driver);
    assert!(
        matches!(&got, SetupRun::Failed { line, exit: Some(1), .. } if line == "b"),
        "{got:?}"
    );
    assert_eq!(reattached(&events), [2]);
}

#[test]
fn a_nonzero_exit_fails_without_attaching_again() {
    let driver = FakeDriver::new()
        .exec_stdout([B_RUNNING])
        .exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let (got, events, _, argvs) = run(driver);
    assert!(
        matches!(&got, SetupRun::Failed { line, exit: Some(1), .. } if line == "b"),
        "{got:?}"
    );
    assert!(reattached(&events).is_empty());
    assert_eq!(argvs.len(), 1);
}

#[test]
fn a_link_down_past_the_wait_fails_naming_the_line_that_was_running() {
    let driver = FakeDriver::new()
        .exec_stdout([B_RUNNING])
        .exec_results(std::iter::once(cut()).chain((0..300).map(|_| unlinked())));
    let (got, events, calls, _) = run(driver);
    assert!(
        matches!(&got, SetupRun::Failed { line, exit: None, reason, .. }
            if line == "b" && reason.contains("machine 1 is not linked")),
        "{got:?}"
    );
    assert_eq!(reattached(&events), [2]);
    assert!(calls.iter().any(|c| c.starts_with("cancel_run")));
}
