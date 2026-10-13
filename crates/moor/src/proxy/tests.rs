//! Tests for the per-Task proxy log.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_core::TaskId;

use super::ProxyLog;
use crate::testing::clock::FakeClock;

/// Opens the log for task 7 at a fixed timestamp.
fn open(state: &Path) -> ProxyLog {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1234)));
    ProxyLog::open(state, TaskId::new(7), clock).unwrap()
}

/// Reads back the log for task 7.
fn read(state: &Path) -> String {
    std::fs::read_to_string(state.join("proxy/7.log")).unwrap()
}

/// Returns the permission bits of `p`.
fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// One record is one `<ts> <method> <route> <status>` line.
#[test]
fn record_writes_line() {
    let t = tempfile::tempdir().unwrap();
    open(t.path()).record("GET", "gh", 200);
    assert_eq!(read(t.path()), "1234 GET gh 200\n");
}

/// A fresh dir is 0700 and a fresh file is 0600.
#[test]
fn fresh_modes() {
    let t = tempfile::tempdir().unwrap();
    let _log = open(t.path());
    assert_eq!(mode(&t.path().join("proxy")), 0o700);
    assert_eq!(mode(&t.path().join("proxy/7.log")), 0o600);
}

/// Looser pre-existing modes are tightened on open.
#[test]
fn existing_modes_tightened() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("proxy");
    let file = dir.join("7.log");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(&file, "").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let _log = open(t.path());
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&file), 0o600);
}

/// Records append rather than truncate.
#[test]
fn records_append() {
    let t = tempfile::tempdir().unwrap();
    let log = open(t.path());
    log.record("GET", "gh", 200);
    log.record("POST", "-", 502);
    assert_eq!(read(t.path()), "1234 GET gh 200\n1234 POST - 502\n");
}

/// Spaces, newlines and non-ASCII bytes never split or widen a line.
#[test]
fn sanitizes_fields() {
    let t = tempfile::tempdir().unwrap();
    open(t.path()).record("G ET\n", "a b\r\nc\u{e9}", 400);
    let text = read(t.path());
    assert_eq!(text, "1234 G_ET_ a_b__c__ 400\n");
    assert_eq!(text.lines().count(), 1);
}
