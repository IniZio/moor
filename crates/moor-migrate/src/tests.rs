use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use rusqlite::Connection;

use sea_query::{ColumnDef, Table};

use super::{
    check, is_latest, migrate, migrations, render, steps, Backend, Error, Legacy, Migration,
    Outcome, Script, Step,
};

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
fn a_file_from_a_newer_build_opens_untouched_and_reports_where_it_stands() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1, V2]), legacy).is_ok());
    let old = set(&[V1]);
    let ahead = Outcome::Ahead { found: 2, known: 1 };
    assert_eq!(check(&conn, &old, legacy).ok(), Some(ahead));
    assert_eq!(migrate(&mut conn, &old, legacy).ok(), Some(ahead));
    assert_eq!(applied(&conn), [1, 2], "nothing applied or removed");
    assert_eq!(count(&conn, "tags"), 0, "the newer tables stay");
    assert!(
        is_latest(&conn, &old).unwrap_or(false),
        "nothing left to migrate"
    );
}

#[test]
fn a_file_behind_the_build_still_migrates() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1]), legacy).is_ok());
    let both = set(&[V1, V2]);
    assert_eq!(check(&conn, &both, legacy).ok(), Some(Outcome::Current));
    assert!(!is_latest(&conn, &both).unwrap_or(true));
    assert_eq!(
        migrate(&mut conn, &both, legacy).ok(),
        Some(Outcome::Current)
    );
    assert_eq!(applied(&conn), [1, 2]);
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
                    migrate(&mut conn, &set, legacy)
                        .map(drop)
                        .map_err(|e| e.to_string())
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

/// V1 and V2 squashed into one migration that starts the set at 3.
const V3: (&str, &str) = ("V3__squashed.sql", "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL); CREATE TABLE tags (note INTEGER NOT NULL REFERENCES notes(id), tag TEXT NOT NULL);");
const V4: (&str, &str) = ("V4__more.sql", "CREATE TABLE more (x TEXT);");

#[test]
fn a_file_through_the_squashed_migrations_is_adopted_at_the_squash() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1, V2]), legacy).is_ok());
    conn.execute("INSERT INTO notes (body) VALUES ('kept')", [])
        .unwrap_or_else(|e| unreachable!("{e}"));
    let squashed = set(&[V3, V4]);
    assert!(check(&conn, &squashed, legacy).is_ok());
    assert!(migrate(&mut conn, &squashed, legacy).is_ok());
    assert_eq!(
        applied(&conn),
        [3, 4],
        "the old history gives way to the squash"
    );
    assert_eq!(count(&conn, "notes"), 1, "existing rows kept");
    assert_eq!(count(&conn, "more"), 0, "the next migration ran");
}

#[test]
fn a_file_short_of_the_squashed_migrations_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V1]), legacy).is_ok());
    let squashed = set(&[V3]);
    let refused = |r: &Result<Outcome, Error>| {
        matches!(
            r,
            Err(Error::Squashed {
                found: 1,
                adopts: 2
            })
        )
    };
    assert!(refused(&check(&conn, &squashed, legacy)));
    assert!(refused(&migrate(&mut conn, &squashed, legacy)));
    assert_eq!(applied(&conn), [1]);
}

#[test]
fn a_fresh_file_runs_the_squashed_set_from_its_first_migration() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    assert!(migrate(&mut conn, &set(&[V3, V4]), legacy).is_ok());
    assert_eq!(applied(&conn), [3, 4]);
    assert_eq!(count(&conn, "tags"), 0, "the squash ran");
}

fn notes(s: &mut Script) {
    s.schema(
        Table::create()
            .table("notes")
            .col(ColumnDef::new("id").big_integer().primary_key())
            .col(ColumnDef::new("body").blob().not_null()),
    );
}

#[test]
fn a_step_renders_once_per_backend_and_runs_as_a_migration() {
    let step = Step {
        name: "V1__notes",
        write: notes,
    };
    assert_eq!(
        render(&step, Backend::Sqlite),
        "CREATE TABLE \"notes\" ( \"id\" integer PRIMARY KEY, \"body\" blob NOT NULL );\n"
    );
    assert_eq!(
        render(&step, Backend::Postgres),
        "CREATE TABLE \"notes\" ( \"id\" bigint PRIMARY KEY, \"body\" bytea NOT NULL );\n"
    );
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("{e}"));
    let mut conn = open(&dir.path().join("a.db"));
    let set = steps(&[step], Backend::Sqlite).unwrap_or_default();
    assert!(migrate(&mut conn, &set, legacy).is_ok());
    assert_eq!(applied(&conn), [1]);
    assert_eq!(count(&conn, "notes"), 0);
}
