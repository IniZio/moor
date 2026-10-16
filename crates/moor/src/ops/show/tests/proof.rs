//! Tests for the proof section of `show`.

use std::sync::Arc;

use moor_core::decide::proof::listing_argv;
use moor_core::seam::driver::{DriverError, ExecOutcome};

use super::super::run;
use super::{add, cx, ready_box, req};
use crate::testing::driver::FakeDriver;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn lists_proof_files_from_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let out = format!("a.txt\t12\t{HASH_A}\nsub/b.png\t3400\t{HASH_B}\n");
    let driver = Arc::new(FakeDriver::new().exec_stdout([out]));
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let files = v.proofs.unwrap();
    let got: Vec<_> = files
        .iter()
        .map(|f| (f.name.as_str(), f.size, f.sha256.as_str()))
        .collect();
    assert_eq!(got, [("a.txt", 12, HASH_A), ("sub/b.png", 3400, HASH_B)]);
    let execs = driver.execs();
    assert_eq!(execs.len(), 1);
    let spec = &execs.first().unwrap().1;
    assert_eq!(spec.argv, listing_argv());
    assert_eq!(spec.sync, moor_core::domain::run::SyncMode::None);
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
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
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
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.proofs.is_none());
    assert_eq!(cx.take_warnings().len(), 1);
}

#[test]
fn a_missing_listing_log_warns() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    ready_box(&cx, &driver, id);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.proofs.is_none());
    let w = cx.take_warnings();
    let codes: Vec<_> = w.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, ["proof_unavailable"]);
}
