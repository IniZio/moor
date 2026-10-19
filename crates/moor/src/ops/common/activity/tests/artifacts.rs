//! Tests for the artifacts collected when a Working task settles to Idle.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::task::TaskState;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::{BoxId, BoxName, ProviderId};
use sha2::{Digest, Sha256};

use super::{reading, row, settle, task};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const BODY: &[u8] = b"hello";

/// A working task whose Box is a live Box of `driver`.
fn fx(driver: FakeDriver) -> (tempfile::TempDir, Arc<FakeDriver>, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(ScriptedProc::new()),
    );
    let id = task(&cx, true);
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let raw = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    raw.execute(
        "UPDATE boxes SET handle = ?1 WHERE id = ?2",
        (got.handle.as_str(), id.get()),
    )
    .unwrap();
    (dir, driver, cx, id)
}

fn listing() -> String {
    format!("notes.txt\t{}\t{:x}\n", BODY.len(), Sha256::digest(BODY))
}

fn settle_quiet(cx: &Cx, id: BoxId) {
    let r = reading(false, true);
    settle(cx, cx.store().unwrap(), &row(cx, id), &r).unwrap();
}

fn published(cx: &Cx, id: BoxId) -> usize {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events
        .iter()
        .filter(|e| e.kind == "artifact_published")
        .count()
}

#[test]
fn settling_to_idle_collects_the_artifacts() {
    let (_dir, driver, cx, id) = fx(FakeDriver::new().exec_stdout([listing()]).copy_text(BODY));
    settle_quiet(&cx, id);
    assert_eq!(row(&cx, id).state, TaskState::Idle);
    assert_eq!(published(&cx, id), 1);
    assert_eq!(cx.store().unwrap().artifacts(id).unwrap().len(), 1);
    assert_eq!(driver.copies().len(), 1);
}

#[test]
fn a_held_box_lock_skips_the_collection() {
    let (_dir, driver, cx, id) = fx(FakeDriver::new().exec_stdout([listing()]).copy_text(BODY));
    let held = BoxLock::acquire(
        cx.clock(),
        cx.state_dir().unwrap(),
        id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    settle_quiet(&cx, id);
    assert_eq!(row(&cx, id).state, TaskState::Working);
    assert!(driver.execs().is_empty());
    assert_eq!(published(&cx, id), 0);
    drop(held);
}

#[test]
fn a_busy_reading_collects_nothing() {
    let (_dir, driver, cx, id) = fx(FakeDriver::new().exec_stdout([listing()]));
    let r = reading(true, false);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    assert!(driver.execs().is_empty());
}

#[test]
fn a_failing_index_exec_still_leaves_the_task_idle() {
    let failure = Err(DriverError::Failed {
        summary: "boom".into(),
    });
    let (_dir, _driver, cx, id) = fx(FakeDriver::new().exec_results([failure]));
    settle_quiet(&cx, id);
    assert_eq!(row(&cx, id).state, TaskState::Idle);
    assert_eq!(published(&cx, id), 0);
    assert!(!cx.take_warnings().is_empty());
}

#[test]
fn a_nonzero_index_exit_still_leaves_the_task_idle() {
    let exit = Ok(ExecOutcome::Exited { code: 2 });
    let (_dir, _driver, cx, id) = fx(FakeDriver::new().exec_results([exit]));
    settle_quiet(&cx, id);
    assert_eq!(row(&cx, id).state, TaskState::Idle);
    assert_eq!(published(&cx, id), 0);
    assert!(!cx.take_warnings().is_empty());
}
