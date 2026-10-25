//! Tests for the front's database.
#![allow(clippy::unwrap_used, reason = "test assertions")]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use moor_core::name::SessionId;
use rusqlite::Connection;
use tempfile::TempDir;

use super::{Db, DbError, RequestRow, BASELINE_VERSION};

fn row(key: &str, created_at: i64) -> RequestRow {
    RequestRow {
        key: key.to_owned(),
        root_session: SessionId::new(7),
        repo: "/work/app".to_owned(),
        created_at,
        ended_at: None,
    }
}

fn open() -> (TempDir, PathBuf, Db) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("web.db");
    let db = Db::open(&path).unwrap();
    (dir, path, db)
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn request_maps_key_to_root_session_and_repo() {
    let (_dir, _path, db) = open();
    let r = row("k1", 10);
    db.insert_request(&r).unwrap();
    assert_eq!(db.request("k1").unwrap(), Some(r));
    assert_eq!(db.request("other").unwrap(), None);
}

#[test]
fn duplicate_key_errors() {
    let (_dir, _path, db) = open();
    db.insert_request(&row("k1", 10)).unwrap();
    let err = db.insert_request(&row("k1", 11)).unwrap_err();
    assert!(matches!(err, DbError::Sqlite(_)));
}

#[test]
fn end_request_sets_ended_at() {
    let (_dir, _path, db) = open();
    db.insert_request(&row("k1", 10)).unwrap();
    assert!(db.end_request("k1", 99).unwrap());
    assert_eq!(db.request("k1").unwrap().unwrap().ended_at, Some(99));
    assert!(!db.end_request("missing", 99).unwrap());
}

#[test]
fn requests_ordered_by_created_at_then_key() {
    let (_dir, _path, db) = open();
    for (k, t) in [("b", 5), ("c", 1), ("a", 5)] {
        db.insert_request(&row(k, t)).unwrap();
    }
    let keys: Vec<_> = db.requests().unwrap().into_iter().map(|r| r.key).collect();
    assert_eq!(keys, ["c", "a", "b"]);
}

#[test]
fn cursor_starts_at_zero_and_persists_across_reopen() {
    let (_dir, path, db) = open();
    assert_eq!(db.cursor().unwrap(), 0);
    db.set_cursor(41).unwrap();
    db.set_cursor(42).unwrap();
    drop(db);
    assert_eq!(Db::open(&path).unwrap().cursor().unwrap(), 42);
}

#[test]
fn reopen_is_idempotent() {
    let (_dir, path, db) = open();
    db.insert_request(&row("k1", 10)).unwrap();
    drop(db);
    let db = Db::open(&path).unwrap();
    assert_eq!(db.requests().unwrap(), [row("k1", 10)]);
    drop(db);
    let applied: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT count(*) FROM refinery_schema_history", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(applied, 1);
}

#[test]
fn newer_schema_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web.db");
    let newer = BASELINE_VERSION + 1;
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", newer)
        .unwrap();
    let err = Db::open(&path).err().unwrap();
    assert!(matches!(err, DbError::NewerSchema { found, .. } if found == newer));
    let tables: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 0);
}

#[test]
fn files_are_0600_and_new_state_dir_is_0700() {
    let (_dir, path, db) = open();
    db.insert_request(&row("k1", 10)).unwrap();
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&path), 0o600);
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let p = PathBuf::from(name);
        if p.exists() {
            assert_eq!(mode(&p), 0o600);
        }
    }
}

#[test]
fn a_file_from_the_last_build_before_migrations_is_adopted_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../../migrations/V1__baseline.sql"))
        .unwrap();
    conn.pragma_update(None, "user_version", BASELINE_VERSION)
        .unwrap();
    conn.execute(
        "INSERT INTO requests (key, root_session, repo, created_at) VALUES ('k1', '7', '/work/app', 10)",
        [],
    )
    .unwrap();
    drop(conn);
    let db = Db::open(&path).unwrap();
    assert_eq!(db.requests().unwrap(), [row("k1", 10)]);
    let applied: i64 = db
        .conn
        .query_row("SELECT count(*) FROM refinery_schema_history", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(applied, 1);
}

#[test]
fn applied_migrations_are_never_edited() {
    let pinned: Vec<(i32, u64)> = super::schema()
        .unwrap()
        .iter()
        .map(|m| (m.version(), m.checksum()))
        .collect();
    assert_eq!(
        pinned,
        [(1, 12_497_078_658_478_940_194)],
        "a schema change is a new migration file; an applied one must not change"
    );
}
