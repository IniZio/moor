//! Tests for the durable artifact-collection cursor.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};

use crate::store::Store;
use crate::testing::clock::FakeClock;

/// Open a store in `dir` with a fake clock.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a Box that is Ready with a handle and no intent.
fn ready(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    store
        .writer()
        .execute(
            "UPDATE boxes SET state = 'ready', handle = 'h' WHERE id = ?1",
            [id.get()],
        )
        .unwrap();
    id
}

/// Run `sql` against the Box row.
fn update(store: &Store, id: BoxId, sql: &str) {
    store.writer().execute(sql, [id.get()]).unwrap();
}

/// Insert a raw event row and return its id.
fn event(store: &Store, id: BoxId, kind: &str) -> i64 {
    let conn = store.writer();
    conn.execute(
        "INSERT INTO events (box, kind, payload, ts) VALUES (?1, ?2, '{}', 0)",
        rusqlite::params![id.get(), kind],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn due_after_idled_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    assert!(store.artifacts_due().unwrap().is_empty());
    let e = event(&store, t, "idled");
    assert_eq!(store.artifacts_due().unwrap(), vec![(t, e)]);
    assert_eq!(store.newest_turn_event(t).unwrap(), Some(e));
}

#[test]
fn turn_done_counts_and_other_kinds_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    event(&store, t, "busy");
    assert!(store.artifacts_due().unwrap().is_empty());
    assert_eq!(store.newest_turn_event(t).unwrap(), None);
    let e = event(&store, t, "turn_done");
    assert_eq!(store.artifacts_due().unwrap(), vec![(t, e)]);
}

#[test]
fn not_due_once_cursor_reaches_newest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    let e = event(&store, t, "idled");
    store.set_artifacts_through(t, e).unwrap();
    assert!(store.artifacts_due().unwrap().is_empty());
    let next = event(&store, t, "turn_done");
    assert_eq!(store.artifacts_due().unwrap(), vec![(t, next)]);
}

#[test]
fn due_lists_boxes_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = ready(&store, "a");
    let b = ready(&store, "b");
    let eb = event(&store, b, "idled");
    let ea = event(&store, a, "idled");
    assert_eq!(store.artifacts_due().unwrap(), vec![(a, ea), (b, eb)]);
}

#[test]
fn not_due_unless_ready_with_handle_and_no_intent() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    event(&store, t, "idled");
    update(
        &store,
        t,
        "UPDATE boxes SET state = 'sleeping' WHERE id = ?1",
    );
    assert!(store.artifacts_due().unwrap().is_empty());
    update(
        &store,
        t,
        "UPDATE boxes SET state = 'ready', handle = NULL WHERE id = ?1",
    );
    assert!(store.artifacts_due().unwrap().is_empty());
    update(
        &store,
        t,
        "UPDATE boxes SET handle = 'h', intent_kind = 'close' WHERE id = ?1",
    );
    assert!(store.artifacts_due().unwrap().is_empty());
    update(
        &store,
        t,
        "UPDATE boxes SET intent_kind = NULL WHERE id = ?1",
    );
    assert_eq!(store.artifacts_due().unwrap().len(), 1);
}

#[test]
fn cursor_never_moves_backwards() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    let first = event(&store, t, "idled");
    let second = event(&store, t, "idled");
    store.set_artifacts_through(t, second).unwrap();
    store.set_artifacts_through(t, first).unwrap();
    assert!(store.artifacts_due().unwrap().is_empty());
}

#[test]
fn reopening_creates_the_cursor_table() {
    let dir = tempfile::tempdir().unwrap();
    let t = {
        let store = open(dir.path());
        let t = ready(&store, "a");
        store
            .writer()
            .execute_batch("DROP TABLE artifact_cursors")
            .unwrap();
        t
    };
    let store = open(dir.path());
    let e = event(&store, t, "idled");
    store.set_artifacts_through(t, e).unwrap();
    assert!(store.artifacts_due().unwrap().is_empty());
}
