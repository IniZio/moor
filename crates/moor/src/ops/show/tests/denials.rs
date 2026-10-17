//! Tests for the sandbox denials section of `show`.

use std::sync::Arc;

use moor_core::seam::driver::{DriverError, ExecOutcome};

use super::super::run;
use super::{add, cx, ready_box, req};
use crate::testing::driver::FakeDriver;

/// Boxes whose workdir lies inside the macOS layout of task `a`.
const MAC_ROOT: &str = "/Users/Shared/moor-n/a/work";

/// Show task `a` over `driver` after giving it a Ready Box; returns the view and the driver.
fn show(driver: FakeDriver) -> (moor_core::view::ShowView, Arc<FakeDriver>) {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    (run(&cx, &req(Some("a"), None)).unwrap(), driver)
}

#[test]
fn mac_task_lists_its_denials() {
    let report = "moor-denials: report\nmoor-denials: log\n\
        2026-01-01 00:00:00.0 Sandbox: git(42) deny(1) file-write-create /Users/Shared/moor-n/a/home/x\n";
    let (v, driver) = show(
        FakeDriver::new()
            .work_root(MAC_ROOT)
            .exec_stdout(["", report]),
    );
    let execs = driver.execs();
    assert_eq!(execs.len(), 2);
    let spec = &execs.last().unwrap().1;
    assert_eq!(spec.argv.first().map(String::as_str), Some("sh"));
    assert_eq!(spec.argv.get(1).map(String::as_str), Some("-c"));
    assert!(spec.argv.get(2).unwrap().contains("/Users/Shared/moor-n/a"));
    let got = v.sandbox_denials.unwrap();
    assert_eq!(
        got,
        ["git: file-write-create /Users/Shared/moor-n/a/home/x"]
    );
}

#[test]
fn linux_task_has_no_denials_and_no_extra_exec() {
    let (v, driver) = show(FakeDriver::new());
    assert!(v.sandbox_denials.is_none());
    assert_eq!(driver.execs().len(), 1);
}

#[test]
fn exec_failure_leaves_denials_none() {
    let (v, _) = show(FakeDriver::new().work_root(MAC_ROOT).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Err(DriverError::NotFound),
    ]));
    assert!(v.sandbox_denials.is_none() && v.task.is_some());
}

#[test]
fn nonzero_exit_leaves_denials_none() {
    let (v, _) = show(FakeDriver::new().work_root(MAC_ROOT).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 1 }),
    ]));
    assert!(v.sandbox_denials.is_none());
}
