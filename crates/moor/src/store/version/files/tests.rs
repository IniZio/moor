//! Tests for the retire lock and the no-replace move.

use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;

use super::{lock_retire, move_noreplace};

#[test]
fn move_noreplace_moves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("a");
    let to = dir.path().join("b");
    fs::write(&from, "data").unwrap();

    move_noreplace(&from, &to).unwrap();

    assert_eq!(fs::read_to_string(&to).unwrap(), "data");
    assert!(!from.exists());
}

#[test]
fn move_noreplace_refuses_an_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("a");
    let to = dir.path().join("b");
    fs::write(&from, "new").unwrap();
    fs::write(&to, "old").unwrap();

    let err = move_noreplace(&from, &to).unwrap_err();

    assert_eq!(err.kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&from).unwrap(), "new");
    assert_eq!(fs::read_to_string(&to).unwrap(), "old");
}

#[test]
fn lock_retire_creates_parent_and_excludes_others_until_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state").join("moor.db");
    let lock_path = dir.path().join("state").join("moor.db.retire.lock");

    let first = lock_retire(&db).unwrap();
    assert!(lock_path.exists());

    let other = OpenOptions::new().write(true).open(&lock_path).unwrap();
    assert!(matches!(
        other.try_lock(),
        Err(fs::TryLockError::WouldBlock)
    ));

    drop(first);
    assert!(other.try_lock().is_ok());
    let _: File = other;
}
