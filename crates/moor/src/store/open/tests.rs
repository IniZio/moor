//! Tests for the store's file-permission handling.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use moor_core::seam::clock::Ts;

use crate::testing::clock::FakeClock;

use super::*;

/// The permission bits of `path`.
fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Create `path` with the given mode.
fn touch(path: &Path, bits: u32) {
    fs::write(path, b"").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(bits)).unwrap();
}

#[test]
fn prepare_creates_private_dir_and_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("state").join("moor.db");
    prepare(&db).unwrap();
    assert_eq!(mode(db.parent().unwrap()), 0o700);
    assert_eq!(mode(&db), 0o600);
}

#[test]
fn prepare_tightens_existing_dir_and_db() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("state");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let db = dir.join("moor.db");
    touch(&db, 0o644);
    prepare(&db).unwrap();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&db), 0o600);
}

#[test]
fn tighten_sets_db_and_wal_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("moor.db");
    let wal = tmp.path().join("moor.db-wal");
    let shm = tmp.path().join("moor.db-shm");
    for p in [&db, &wal, &shm] {
        touch(p, 0o644);
    }
    tighten(&db).unwrap();
    for p in [&db, &wal, &shm] {
        assert_eq!(mode(p), 0o600);
    }
}

#[test]
fn tighten_skips_absent_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("moor.db");
    touch(&db, 0o644);
    tighten(&db).unwrap();
    assert_eq!(mode(&db), 0o600);
}

#[test]
fn store_open_leaves_everything_private() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("state");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let db = dir.join("moor.db");
    touch(&db, 0o644);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let _store = super::super::Store::open(&db, clock).unwrap();
    assert_eq!(mode(&dir), 0o700);
    for name in ["moor.db", "moor.db-wal", "moor.db-shm"] {
        assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
    }
}
