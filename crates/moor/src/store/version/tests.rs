//! Tests for retiring an old-format state database.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use rusqlite::Connection;

use super::retire_v1;
use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;

/// Creates a database at `dir/moor.db` holding a `meta` row and a `tasks` table.
fn make_db(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join("moor.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);")
        .unwrap();
    conn.execute(
        "INSERT INTO meta (k, v) VALUES ('schema_version', ?1)",
        [version],
    )
    .unwrap();
    conn.execute_batch("CREATE TABLE tasks (id INTEGER PRIMARY KEY);")
        .unwrap();
    path
}

/// The path with `suffix` appended to its file name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// True when `db` has a table called `name`.
fn has_table(db: &Path, name: &str) -> bool {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

#[test]
fn no_database_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("moor.db");
    assert_eq!(retire_v1(&db).unwrap(), None);
    assert!(!db.exists());
    File::create(&db).unwrap();
    assert_eq!(retire_v1(&db).unwrap(), None);
    assert!(db.exists());
}

#[test]
fn v1_database_is_moved_to_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    let bak = retire_v1(&db).unwrap().unwrap();
    assert_eq!(bak, suffixed(&db, ".v1.bak"));
    assert!(has_table(&bak, "tasks"));
    assert!(!db.exists());
    assert!(!suffixed(&db, "-wal").exists());
    assert!(!suffixed(&db, "-shm").exists());
}

#[test]
fn second_backup_takes_a_numbered_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    let first = retire_v1(&db).unwrap().unwrap();
    make_db(dir.path(), "1");
    let second = retire_v1(&db).unwrap().unwrap();
    assert_eq!(second, suffixed(&db, ".v1.bak.1"));
    make_db(dir.path(), "1");
    let third = retire_v1(&db).unwrap().unwrap();
    assert_eq!(third, suffixed(&db, ".v1.bak.2"));
    assert!(first.exists() && second.exists() && third.exists());
}

#[test]
fn other_version_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "7");
    assert_eq!(retire_v1(&db).unwrap(), None);
    assert!(has_table(&db, "tasks"));
    assert!(!suffixed(&db, ".v1.bak").exists());
}

#[test]
fn current_version_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "2");
    assert_eq!(retire_v1(&db).unwrap(), None);
    assert!(has_table(&db, "tasks"));
}

#[test]
fn running_controller_refuses_and_keeps_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    let mut lock = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(dir.path().join("controller.sock.lock"))
        .unwrap();
    writeln!(lock, "1\nsock").unwrap();
    lock.try_lock().unwrap();
    let err = retire_v1(&db).unwrap_err();
    assert!(
        matches!(&err, StoreError::ControllerRunning { pid } if pid == "1"),
        "{err:?}"
    );
    assert!(has_table(&db, "tasks"));
    assert!(!suffixed(&db, ".v1.bak").exists());
}

#[test]
fn store_open_on_v1_creates_fresh_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&db, clock).unwrap();
    drop(store);
    let version: String = Connection::open(&db)
        .unwrap()
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(version, "2");
    assert!(suffixed(&db, ".v1.bak").exists());
}
