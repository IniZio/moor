//! The file `run` keeps while it holds a link counts only while the process that wrote it lives.
#![allow(clippy::unwrap_used, reason = "tests")]

use moor_machine_agent::linked::{live, start_of, write};
use moor_testkit::ChildGuard;
use serde_json::json;

#[test]
fn a_mark_written_by_a_live_process_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked");
    write(&path, "https://moor.example.com").unwrap();
    assert!(live(&path));
}

#[test]
fn a_mark_of_a_process_that_has_ended_is_not_live() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked");
    let mut child = ChildGuard::new(std::process::Command::new("true").spawn().unwrap());
    let pid = child.id();
    let start = start_of(pid);
    child.wait().unwrap();
    let mark = json!({"server": "s", "pid": pid, "start": start.unwrap_or_default()});
    std::fs::write(&path, mark.to_string()).unwrap();
    assert!(!live(&path));
}

#[test]
fn a_mark_from_another_boot_is_not_live() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked");
    let mark = json!({"server": "s", "pid": std::process::id(),
        "start": "00000000-0000-0000-0000-000000000000 1"});
    std::fs::write(&path, mark.to_string()).unwrap();
    assert!(!live(&path));
}

#[test]
fn a_missing_or_unreadable_mark_is_not_live() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked");
    assert!(!live(&path));
    std::fs::write(&path, "https://moor.example.com").unwrap();
    assert!(!live(&path));
}
