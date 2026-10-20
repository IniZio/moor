//! Tests for retiring an old-format state database.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use moor_core::seam::clock::Ts;
use rusqlite::Connection;

use super::retire_v1;
use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;

/// Creates a database at `dir/moor.db` holding a `meta` row and a `boxes` table.
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
    conn.execute_batch("CREATE TABLE boxes (id INTEGER PRIMARY KEY);")
        .unwrap();
    path
}

/// Retires `db` and returns the backup path, releasing the lock at once.
fn retire(db: &Path) -> Result<Option<PathBuf>, StoreError> {
    retire_v1(db).map(|r| r.backup)
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
    assert_eq!(retire(&db).unwrap(), None);
    assert!(!db.exists());
    File::create(&db).unwrap();
    assert_eq!(retire(&db).unwrap(), None);
    assert!(db.exists());
}

#[test]
fn v1_database_is_moved_to_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    let bak = retire(&db).unwrap().unwrap();
    assert_eq!(bak, suffixed(&db, ".v1.bak"));
    assert!(has_table(&bak, "boxes"));
    assert!(!db.exists());
    assert!(!suffixed(&db, "-wal").exists());
    assert!(!suffixed(&db, "-shm").exists());
}

#[test]
fn second_backup_takes_a_numbered_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    let first = retire(&db).unwrap().unwrap();
    make_db(dir.path(), "1");
    let second = retire(&db).unwrap().unwrap();
    assert_eq!(second, suffixed(&db, ".v1.bak.1"));
    make_db(dir.path(), "1");
    let third = retire(&db).unwrap().unwrap();
    assert_eq!(third, suffixed(&db, ".v1.bak.2"));
    assert!(first.exists() && second.exists() && third.exists());
}

#[test]
fn other_version_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "7");
    assert_eq!(retire(&db).unwrap(), None);
    assert!(has_table(&db, "boxes"));
    assert!(!suffixed(&db, ".v1.bak").exists());
}

#[test]
fn current_version_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "2");
    assert_eq!(retire(&db).unwrap(), None);
    assert!(has_table(&db, "boxes"));
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
    let err = retire(&db).unwrap_err();
    assert!(
        matches!(&err, StoreError::ControllerRunning { pid } if pid == "1"),
        "{err:?}"
    );
    assert!(has_table(&db, "boxes"));
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
    assert_eq!(version, "4");
    assert!(suffixed(&db, ".v1.bak").exists());
}

/// Puts a v1 database into WAL mode with `rows` rows and returns an idle
/// connection that keeps the `-wal` and `-shm` files alive.
fn make_wal_db(dir: &Path, rows: i64) -> (PathBuf, Connection) {
    let db = make_db(dir, "1");
    let conn = Connection::open(&db).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    for id in 1..=rows {
        conn.execute("INSERT INTO boxes (id) VALUES (?1)", [id])
            .unwrap();
    }
    (db, conn)
}

/// Number of rows in `boxes`.
fn box_count(db: &Path) -> i64 {
    Connection::open(db)
        .unwrap()
        .query_row("SELECT count(*) FROM boxes", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn busy_reader_refuses_and_keeps_database_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (db, writer) = make_wal_db(dir.path(), 3);
    let reader = Connection::open(&db).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM boxes;")
        .unwrap();
    for id in 10..15 {
        writer
            .execute("INSERT INTO boxes (id) VALUES (?1)", [id])
            .unwrap();
    }
    let err = retire(&db).unwrap_err();
    assert!(matches!(err, StoreError::StoreInUse), "{err:?}");
    assert!(!suffixed(&db, ".v1.bak").exists());
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    drop(writer);
    assert_eq!(box_count(&db), 8);
}

#[test]
fn stale_controller_lock_does_not_block_retire() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path(), "1");
    std::fs::write(dir.path().join("controller.sock.lock"), "999999\nsock").unwrap();
    let bak = retire(&db).unwrap().unwrap();
    assert!(has_table(&bak, "boxes"));
}

#[test]
fn concurrent_opens_keep_one_backup_and_a_v2_store() {
    for _ in 0..20 {
        let dir = tempfile::tempdir().unwrap();
        let db = make_db(dir.path(), "1");
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (db, barrier) = (db.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
                    barrier.wait();
                    Store::open(&db, clock).map(drop)
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let bak = suffixed(&db, ".v1.bak");
        assert!(bak.exists());
        assert!(!suffixed(&db, ".v1.bak.1").exists());
        assert!(has_table(&bak, "boxes"));
        let version: String = Connection::open(&db)
            .unwrap()
            .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, crate::store::SCHEMA_VERSION);
    }
}

#[test]
fn wal_and_shm_move_beside_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (db, idle) = make_wal_db(dir.path(), 4);
    assert!(suffixed(&db, "-wal").exists() && suffixed(&db, "-shm").exists());
    let bak = retire(&db).unwrap().unwrap();
    assert!(suffixed(&bak, "-wal").exists());
    assert!(suffixed(&bak, "-shm").exists());
    assert!(!suffixed(&db, "-wal").exists());
    assert!(!suffixed(&db, "-shm").exists());
    drop(idle);
    assert_eq!(box_count(&bak), 4);
}
