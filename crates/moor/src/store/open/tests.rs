//! Tests for the store's file-permission handling and its file locks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::BoxName;

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

/// The mode of `dir`, then each entry's name, mode and a hash of its bytes.
fn snapshot(dir: &Path) -> (u32, Vec<(String, u32, u64)>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&fs::read(&path).unwrap(), &mut hasher);
            (name, mode(&path), std::hash::Hasher::finish(&hasher))
        })
        .collect();
    entries.sort();
    (mode(dir), entries)
}

#[test]
fn store_open_leaves_a_db_at_another_schema_version_as_it_found_it() {
    for journal in ["DELETE", "WAL"] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let db = dir.join("moor.db");
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA journal_mode = {journal};
                 CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
                 INSERT INTO meta (k, v) VALUES ('schema_version', '4');"
            ))
            .unwrap();
        fs::set_permissions(&db, fs::Permissions::from_mode(0o644)).unwrap();
        let before = snapshot(&dir);
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        let opened = super::super::Store::open(&db, clock);
        assert!(
            matches!(opened, Err(super::super::StoreError::Schema { .. })),
            "{journal}: {:?}",
            opened.err()
        );
        assert_eq!(snapshot(&dir), before, "{journal}");
    }
}

/// Names the db that a child process probes for a lock and opens a connection to.
const CHILD_DB: &str = "MOOR_TEST_STORE_CHILD_DB";

/// In a child process: fails when the db takes an exclusive lock, which an `SQLite` closing its
/// last connection takes before it checkpoints and deletes the WAL; then opens and closes one.
#[test]
fn child_finds_the_db_locked_and_closes_a_connection() {
    let Some(db) = std::env::var_os(CHILD_DB) else {
        return;
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&db)
        .unwrap();
    let locked =
        rustix::fs::fcntl_lock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive);
    assert_eq!(
        locked,
        Err(rustix::io::Errno::AGAIN),
        "another process locked a live store's db exclusively"
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    let _: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
}

#[test]
fn a_second_open_keeps_the_first_store_locked_against_other_processes() {
    if std::env::var_os(CHILD_DB).is_some() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("state").join("moor.db");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = super::super::Store::open(&db, clock.clone()).unwrap();
    let insert = |name: &str| {
        Decision::insert(NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: None,
        })
    };
    store.apply(insert("a")).unwrap();
    drop(super::super::Store::open(&db, clock).unwrap());
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::open::tests::child_finds_the_db_locked_and_closes_a_connection",
        ])
        .env(CHILD_DB, &db)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(child.success(), "child: {child}");
    assert!(
        sibling(&db, "-wal").exists(),
        "the WAL of a live store is gone"
    );
    store.apply(insert("b")).unwrap();
    let check: String = store
        .writer()
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
}
