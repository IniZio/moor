//! Tests of what the `SQLite` file holds and how its locks behave: its journal mode, a schema
//! version from before migrations, a reader beside an exclusive writer, and the planner.
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;

use crate::store::tests::{new_box, open};
use crate::store::Store;
use crate::testing::clock::FakeClock;

#[test]
fn open_creates_wal_file_with_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    drop(open(dir.path()));
    assert!(path.exists());
    let raw = rusqlite::Connection::open(&path).unwrap();
    let mode: String = raw
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let v: String = raw
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(v, "16");
    drop(raw);
    let (store, _) = open(dir.path());
    assert!(store.open_boxes().unwrap().is_empty());
}

#[test]
fn a_state_dir_from_another_schema_version_is_refused_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    let (store, _) = open(dir.path());
    new_box(&store, "kept");
    drop(store);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "UPDATE meta SET v = '4' WHERE k = 'schema_version'; DROP TABLE refinery_schema_history;",
        )
        .unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let err = Store::open(&path, clock).err().unwrap().to_string();
    assert!(
        err.contains("use a fresh state dir") && err.contains(&path.display().to_string()),
        "{err}"
    );
    let raw = rusqlite::Connection::open(&path).unwrap();
    let kept: (String, String) = raw
        .query_row(
            "SELECT (SELECT v FROM meta WHERE k = 'schema_version'), (SELECT name FROM boxes)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kept, ("4".to_owned(), "kept".to_owned()));
}

#[test]
fn a_state_dir_from_before_the_cloned_event_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    drop(open(dir.path()));
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "UPDATE meta SET v = '9' WHERE k = 'schema_version'; DROP TABLE refinery_schema_history;",
        )
        .unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let err = Store::open(&path, clock).err().unwrap().to_string();
    assert!(err.contains("use a fresh state dir"), "{err}");
}

#[test]
fn readers_are_not_blocked_by_an_open_writer() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "t");
    let path = dir.path().join("state").join("moor.db");
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "BEGIN EXCLUSIVE; UPDATE boxes SET prompt = 'dirty'; \
         INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES ('x', 'o/r', 'checking', 0, 0); \
         INSERT INTO sessions (box, ordinal, state, started_at) \
         VALUES (last_insert_rowid(), 1, 'starting', 0); \
         INSERT INTO events (box, kind, payload, ts) VALUES (1, 'busy', '{}', 0);",
    )
    .unwrap();
    let start = Instant::now();
    assert_eq!(store.box_row(id).unwrap().unwrap().prompt, None);
    assert_eq!(store.open_boxes().unwrap().len(), 1);
    assert_eq!(store.events(id, 100).unwrap().len(), 1);
    assert!(start.elapsed() < Duration::from_secs(1));
    raw.execute_batch("COMMIT").unwrap();
    assert_eq!(store.open_boxes().unwrap().len(), 2);
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close { reason: None })
                .session(SessionInput::End),
        )
        .unwrap();
}

#[test]
fn waiting_boxes_returns_pending_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let d = new_box(&store, "d");
    let b = new_box(&store, "b");
    new_box(&store, "c");
    let a = new_box(&store, "a");
    assert!(store.waiting_boxes().unwrap().is_empty());
    // An index on (state, name) lets the planner return rows in name order.
    store
        .writer()
        .execute_batch("CREATE INDEX boxes_state_name ON boxes(state, name)")
        .unwrap();
    for id in [d, b, a] {
        let input = BoxLifecycleInput::AtCapacity { max_boxes: 1 };
        store.apply(Decision::new(id).lifecycle(input)).unwrap();
    }
    let waiting = store.waiting_boxes().unwrap();
    assert_eq!(waiting.iter().map(|t| t.id).collect::<Vec<_>>(), [d, b, a]);
    assert!(waiting.iter().all(|t| t.lifecycle == BoxLifecycle::Pending));
}
