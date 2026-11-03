//! A populated store at the baseline keeps its rows and their order through the migration that
//! gives `bindings` and `box_refs` an `id`.

use std::path::Path;
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_migrate::{Backend, Legacy};
use rusqlite::Connection;

use crate::store::Store;
use crate::testing::clock::FakeClock;

/// A db at the baseline migration only, holding two Boxes, three bindings of one key and three
/// branch refs, each inserted out of key order so rowid order differs from key order.
fn baseline_store(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
        .unwrap();
    let baseline =
        moor_migrate::steps(moor_schema::STEPS.get(..1).unwrap(), Backend::Sqlite).unwrap();
    moor_migrate::migrate(&mut conn, &baseline, |_| Ok(Legacy::Empty)).unwrap();
    conn.execute_batch(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('a', 'r', 'ready', 1, 1);
         INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('b', 'r', 'ready', 1, 1);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (2, 1, 'ended', 1, 2);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 1, 'ended', 1, 2);
         INSERT INTO sessions (box, ordinal, state, started_at, ended_at) VALUES (1, 2, 'ended', 3, 4);
         INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('cli', 'k', 3, 'U', 5);
         INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('cli', 'k', 1, 'U', 5);
         INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('cli', 'k', 2, 'U', 4);
         INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'zeta');
         INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'alpha');
         INSERT INTO box_refs (box, kind, value) VALUES (1, 'branch', 'mid');
         INSERT INTO box_refs (box, kind, value) VALUES (2, 'pr', '7');",
    )
    .unwrap();
}

/// The first column of each row `sql` returns, as text.
fn strings(path: &Path, sql: &str) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

/// Both tables' rows in the order the store reads them, ordering by `seq`.
fn ordered(path: &Path, seq: &str) -> (Vec<String>, Vec<String>) {
    (
        strings(
            path,
            &format!(
                "SELECT front || '/' || conv_key || '/' || session || '/' || owner || '/' || \
                 created_at FROM bindings ORDER BY created_at DESC, {seq} DESC"
            ),
        ),
        strings(
            path,
            &format!("SELECT box || '/' || kind || '/' || value FROM box_refs ORDER BY {seq}"),
        ),
    )
}

fn open(path: &Path) -> Store {
    Store::open(path, Arc::new(FakeClock::new(Ts::from_millis(10)))).unwrap()
}

#[test]
fn a_populated_baseline_store_keeps_its_rows_in_insertion_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    baseline_store(&path);
    let before = ordered(&path, "rowid");
    let newest = open(&path).binding("cli", "k").unwrap().unwrap().session;
    assert_eq!(newest, 1, "ties on created_at go to the later insert");
    assert_eq!(ordered(&path, "id"), before);
    let store = open(&path);
    assert_eq!(
        store.box_refs(BoxId::new(1), "branch").unwrap(),
        ["zeta", "alpha", "mid"]
    );
    assert!(!store.add_box_ref(BoxId::new(1), "branch", "alpha").unwrap());
    assert!(store.add_box_ref(BoxId::new(1), "nonsense", "x").is_err());
    assert!(store.add_box_ref(BoxId::new(1), "branch", "last").unwrap());
    assert_eq!(
        store.box_refs(BoxId::new(1), "branch").unwrap(),
        ["zeta", "alpha", "mid", "last"]
    );
}

#[test]
fn the_migration_runs_once_and_a_reopen_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    baseline_store(&path);
    drop(open(&path));
    let history = |path: &Path| {
        strings(
            path,
            "SELECT CAST(version AS TEXT) FROM refinery_schema_history ORDER BY version",
        )
    };
    let first = (history(&path), rows_after(&path));
    drop(open(&path));
    assert_eq!((history(&path), rows_after(&path)), first);
    assert_eq!(first.0, ["45", "46"]);
}

/// Every row of both tables with its `id`.
fn rows_after(path: &Path) -> Vec<String> {
    strings(
        path,
        "SELECT 'g/' || id || '/' || session FROM bindings \
         UNION ALL SELECT 'r/' || id || '/' || value FROM box_refs ORDER BY 1",
    )
}
