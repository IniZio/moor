use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use rusqlite::Connection;

use super::{check, migrate, migrations, Error, Legacy, Migration};

const V1: (&str, &str) = (
    "V1__baseline.sql",
    "CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL); \
     INSERT INTO meta VALUES ('schema_version', '7'); \
     CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);",
);
const V2: (&str, &str) = (
    "V2__tags.sql",
    "CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes(id), tag TEXT NOT NULL);",
);

/// The old-version marker a pre-migration build kept in `meta`.
fn legacy(conn: &Connection) -> rusqlite::Result<Legacy> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(Legacy::Empty);
    }
    let v: String = conn.query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
        r.get(0)
    })?;
    Ok(if v == "7" {
        Legacy::Baseline
    } else {
        Legacy::Other(v)
    })
}

fn set(files: &[(&str, &str)]) -> Vec<Migration> {
    migrations(files).unwrap_or_default()
}

fn open(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap_or_else(|e| unreachable!("open: {e}"));
    conn.busy_timeout(Duration::from_secs(10))
        .unwrap_or_else(|e| unreachable!("busy: {e}"));
    conn.execute_batch("PRAGMA journal_mode = WAL")
        .unwrap_or_else(|e| unreachable!("wal: {e}"));
    conn
}

fn applied(conn: &Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT version FROM refinery_schema_history ORDER BY version")
        .unwrap_or_else(|e| unreachable!("prepare: {e}"));
    let rows = stmt
        .query_map([], |r| r.get(0))
        .unwrap_or_else(|e| unreachable!("query: {e}"));
    rows.collect::<Result<_, _>>()
        .unwrap_or_else(|e| unreachable!("rows: {e}"))
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap_or_else(|e| unreachable!("count {table}: {e}"))
}

#[test]
fn a_fresh_file_runs_every_migration() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1, V2]), legacy).is_ok());
    assert_eq!(applied(&conn), [1, 2]);
    assert_eq!(count(&conn, "tags"), 0, "migration 2 ran");
}

#[test]
fn a_baseline_file_is_adopted_unchanged_then_takes_the_next_migration() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    conn.execute_batch(V1.1)
        .unwrap_or_else(|e| unreachable!("{e}"));
    conn.execute("INSERT INTO notes (body) VALUES ('kept')", [])
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(migrate(&mut conn, &set(&[V1]), legacy).is_ok());
    assert_eq!(applied(&conn), [1], "adopted without running the baseline");
    assert_eq!(count(&conn, "notes"), 1, "existing rows kept");
    assert!(migrate(&mut conn, &set(&[V1, V2]), legacy).is_ok());
    assert_eq!(applied(&conn), [1, 2]);
    assert_eq!(count(&conn, "notes"), 1, "existing rows kept");
}

#[test]
fn another_old_version_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    conn.execute_batch(&V1.1.replace("'7'", "'6'"))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let s = set(&[V1]);
    assert!(matches!(check(&conn, &s, legacy), Err(Error::Unadoptable(v)) if v == "6"));
    assert!(matches!(migrate(&mut conn, &s, legacy), Err(Error::Unadoptable(v)) if v == "6"));
    let history: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'refinery_schema_history')",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(!history, "a refused file gains no history table");
}

#[test]
fn a_file_from_a_newer_build_is_refused() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1, V2]), legacy).is_ok());
    let old = set(&[V1]);
    let newer = |r: &Result<(), Error>| matches!(r, Err(Error::Newer { found: 2, known: 1 }));
    assert!(newer(&check(&conn, &old, legacy)));
    assert!(newer(&migrate(&mut conn, &old, legacy)));
}

#[test]
fn an_edited_applied_migration_is_refused() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1]), legacy).is_ok());
    let edited = (V1.0, &*format!("{} -- edited", V1.1));
    assert!(matches!(
        migrate(&mut conn, &set(&[edited]), legacy),
        Err(Error::Migration(_))
    ));
}

#[test]
fn two_openers_racing_on_a_new_file_migrate_it_once() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let path = dir.path().join("a.db");
    let set = Arc::new(set(&[V1, V2]));
    for _ in 0..20 {
        let _ = std::fs::remove_file(&path);
        drop(open(&path));
        let gate = Arc::new(Barrier::new(2));
        let racers: Vec<_> = (0..2)
            .map(|_| {
                let (path, gate, set) = (path.clone(), gate.clone(), set.clone());
                std::thread::spawn(move || {
                    let mut conn = open(&path);
                    gate.wait();
                    migrate(&mut conn, &set, legacy).map_err(|e| e.to_string())
                })
            })
            .collect();
        for racer in racers {
            let result = racer
                .join()
                .unwrap_or_else(|_| unreachable!("racer panicked"));
            assert_eq!(result, Ok(()), "both openers succeed");
        }
        assert_eq!(
            applied(&open(&path)),
            [1, 2],
            "each migration recorded once"
        );
    }
}
