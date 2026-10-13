use std::fs;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::Decision;
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::TaskState;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::ErrorKind;

use super::{box_state, fx_with, req, row, run, Fx};
use crate::ops::common::boxlock::{BoxLock, WAIT};
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

pub(super) const YAML: &str =
    "teardown: ['echo bye', 'rm -rf cache']\nenv: {K: v}\nbox: {size: large}\n";

fn fx_yaml(driver: FakeDriver, yaml: &str) -> Fx {
    let f = fx_with(driver, FakeSurface::new("/s"), 0);
    fs::write(f.dir.path().join(".moor.yaml"), yaml).unwrap();
    f
}

pub(super) fn exec_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("exec"))
        .count()
}

/// The single exec the close made.
fn only_exec(f: &Fx) -> ExecSpec {
    let mut execs = f.driver.execs();
    assert_eq!(execs.len(), 1);
    execs.remove(0).1
}

#[test]
fn teardown_runs_as_one_synced_script_before_release() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    let spec = only_exec(&f);
    assert!(matches!(spec.argv.as_slice(), [sh, c, script]
        if sh == "sh" && c == "-c" && script.contains("echo bye") && script.contains("rm -rf cache")));
    assert_eq!(spec.sync, SyncMode::Full);
    assert_eq!(spec.host_cwd.as_deref(), Some(f.dir.path()));
    assert_eq!(spec.class.as_deref(), Some("large"));
    assert_eq!(spec.env, [("K".to_owned(), "v".to_owned())]);
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn teardown_runs_before_the_box_is_released() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    run(&f.cx, &req(false)).unwrap();
    let calls = f.driver.calls();
    let at = |p: &str| calls.iter().position(|c| c.starts_with(p)).unwrap();
    assert!(at("exec") < at("release"));
}

#[test]
fn failing_teardown_warns_and_still_releases() {
    let f = fx_yaml(
        FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 3 })]),
        YAML,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(v.task.state, TaskState::Closed);
    let warnings = f.cx.take_warnings();
    assert!(warnings.iter().any(|w| w.code == "teardown_failed"));
}

#[test]
fn teardown_driver_error_warns_and_still_releases() {
    let f = fx_yaml(
        FakeDriver::new().exec_results([Err(DriverError::Failed {
            summary: "boom".into(),
        })]),
        YAML,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "teardown_failed"));
}

#[test]
fn no_teardown_lines_runs_nothing() {
    let f = fx_yaml(FakeDriver::new(), "env: {K: v}\n");
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(exec_calls(&f), 0);
}

#[test]
fn gone_box_skips_teardown() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(exec_calls(&f), 0);
    assert_eq!(row(&f).state, TaskState::Closed);
}

#[test]
fn refused_close_runs_no_teardown() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 2);
    fs::write(f.dir.path().join(".moor.yaml"), YAML).unwrap();
    assert_eq!(
        run(&f.cx, &req(false)).unwrap_err().kind,
        ErrorKind::Refused
    );
    assert_eq!(exec_calls(&f), 0);
}

#[test]
fn close_waits_for_the_box_lock_then_refuses_when_busy() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    let _held =
        BoxLock::acquire(f.cx.clock(), f.cx.state_dir().unwrap(), f.id, "run", WAIT).unwrap();
    let from = f.cx.clock().instant();
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(e.error.to_string().contains("run"), "{}", e.error);
    assert!(f.cx.clock().instant() - from >= WAIT);
    assert_eq!(exec_calls(&f), 0);
    assert_ne!(row(&f).state, TaskState::Closed);
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn close_leaves_the_box_lock_free() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    run(&f.cx, &req(false)).unwrap();
    let again = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "start",
        std::time::Duration::ZERO,
    );
    assert!(again.is_ok());
}

#[test]
fn asleep_box_is_released_without_teardown() {
    let f = fx_yaml(FakeDriver::new(), YAML);
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::Sleep))
        .unwrap();
    assert_eq!(box_state(&f), BoxState::Asleep);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(exec_calls(&f), 0);
}

#[test]
fn invalid_repo_config_warns_and_still_closes_without_teardown() {
    let f = fx_yaml(FakeDriver::new(), "teardown: [\n");
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(exec_calls(&f), 0);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "teardown_failed"));
}
