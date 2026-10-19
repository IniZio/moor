//! Tests for the proof section of `show`.

use std::sync::Arc;

use moor_core::seam::driver::{DriverError, ExecOutcome};

use super::super::run;
use super::{add, cx, ready_box, req, working};
use crate::testing::driver::FakeDriver;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn lists_proof_files_from_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let out = format!("a.txt\t12\t{HASH_A}\nsub/b.png\t3400\t{HASH_B}\n");
    let driver = Arc::new(FakeDriver::new().run_stdout(&out));
    let cx = cx(dir.path(), &driver);
    working(&cx, &driver, "a", None);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let files = v.proofs.unwrap();
    let got: Vec<_> = files
        .iter()
        .map(|f| (f.name.as_str(), f.size, f.sha256.as_str()))
        .collect();
    assert_eq!(got, [("a.txt", 12, HASH_A), ("sub/b.png", 3400, HASH_B)]);
    assert_eq!(
        driver.box_calls().iter().filter(|c| *c == "batch").count(),
        1
    );
}

#[test]
fn no_box_means_no_proofs() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver);
    add(&cx, "a");
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.proofs.is_none());
    assert!(driver.execs().is_empty());
    assert!(cx.take_warnings().is_empty());
}

#[test]
fn exec_failure_warns_and_still_shows() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().exec_results([Err(DriverError::NotFound)]));
    let cx = cx(dir.path(), &driver);
    working(&cx, &driver, "a", None);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.proofs.is_none() && v.task.is_some());
    let w = cx.take_warnings();
    let codes: Vec<_> = w.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, ["proof_unavailable"]);
}

#[test]
fn nonzero_exit_warns() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 2 })]));
    let cx = cx(dir.path(), &driver);
    working(&cx, &driver, "a", None);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.proofs.is_none());
    assert_eq!(cx.take_warnings().len(), 1);
}

#[test]
fn an_unrecorded_workdir_warns_without_touching_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.proofs.is_none() && v.task.is_some());
    let w = cx.take_warnings();
    let codes: Vec<_> = w.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, ["proof_unavailable"]);
    assert!(w
        .first()
        .unwrap()
        .message
        .contains("workdir is not recorded"));
    assert!(!driver
        .box_calls()
        .iter()
        .any(|c| c == "batch" || c == "exec"));
    assert!(driver.execs().is_empty());
}

#[test]
fn show_lists_proofs_only_inside_a_batch() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().run_stdout(""));
    let cx = cx(dir.path(), &driver);
    working(&cx, &driver, "a", None);
    let before = driver.box_calls().len();
    run(&cx, &req(Some("a"), None)).unwrap();
    let calls = driver.box_calls();
    let shown = calls.get(before..).unwrap();
    assert!(shown.iter().any(|c| c == "batch"));
    assert!(!shown.iter().any(|c| c == "exec"));
}

#[test]
fn lists_proof_files_from_the_task_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().run_stdout(""));
    let cx = cx(dir.path(), &driver);
    working(&cx, &driver, "a", None);
    run(&cx, &req(Some("a"), None)).unwrap();
    let execs = driver.execs();
    assert_eq!(
        execs.first().unwrap().1.host_cwd.as_deref(),
        Some(std::path::Path::new("/wt"))
    );
}
