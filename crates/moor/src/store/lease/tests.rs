//! Tests for the Box lease records.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{TaskId, TaskName};

use super::{retry_after, LeaseKind, LeaseState, ProcessIds};
use crate::store::Store;
use crate::testing::clock::FakeClock;

const ALL: [LeaseKind; 9] = [
    LeaseKind::BoxRoot,
    LeaseKind::BoxDir,
    LeaseKind::Home,
    LeaseKind::Xcconfig,
    LeaseKind::SimSet,
    LeaseKind::SimDevice,
    LeaseKind::Process,
    LeaseKind::Compose,
    LeaseKind::Cache,
];

fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

fn add(store: &Store, name: &str) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

#[test]
fn plan_then_hold() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::BoxDir, "/b/a").unwrap();
    let rows = store.leases(t).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].state, LeaseState::Planned);
    assert_eq!(rows[0].key, "/b/a");
    assert!(!rows[0].booted);
    assert!(store.hold_lease(id).unwrap());
    assert_eq!(store.leases(t).unwrap()[0].state, LeaseState::Held);
}

#[test]
fn plan_is_idempotent_and_keeps_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::Home, "/h").unwrap();
    store.hold_lease(id).unwrap();
    let again = store.plan_lease(t, LeaseKind::Home, "/h").unwrap();
    assert_eq!(again, id);
    let rows = store.leases(t).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, LeaseState::Held);
}

#[test]
fn failed_creation_leaves_planned_row_that_can_be_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::SimDevice, "UDID-1").unwrap();
    // Creation fails here: hold_lease is never called.
    let rows = store.leases(t).unwrap();
    assert_eq!(rows[0].state, LeaseState::Planned);
    assert!(store.drop_lease(id).unwrap());
    assert!(store.leases(t).unwrap().is_empty());
    assert!(!store.drop_lease(id).unwrap());
}

#[test]
fn foreign_key_rejects_unknown_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let err = store.plan_lease(TaskId::new(999), LeaseKind::BoxDir, "/x");
    assert!(err.is_err());
    assert!(store.all_leases().unwrap().is_empty());
}

#[test]
fn rows_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (t, id) = {
        let store = open(dir.path());
        let t = add(&store, "a");
        let id = store.plan_lease(t, LeaseKind::Process, "tag-1").unwrap();
        store.hold_lease(id).unwrap();
        store.set_booted(id, true).unwrap();
        let ids = ProcessIds {
            pgid: 10,
            sid: 11,
            root_pid: 12,
            root_start: 13,
        };
        assert!(store.record_process(id, ids).unwrap());
        (t, id)
    };
    let store = open(dir.path());
    let rows = store.leases(t).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].state, LeaseState::Held);
    assert!(rows[0].booted);
    assert_eq!(
        rows[0].process,
        Some(ProcessIds {
            pgid: 10,
            sid: 11,
            root_pid: 12,
            root_start: 13
        })
    );
    assert_eq!(store.all_leases().unwrap().len(), 1);
}

#[test]
fn kinds_round_trip() {
    for kind in ALL {
        assert_eq!(LeaseKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(LeaseKind::parse("nope"), None);
    assert_eq!(LeaseKind::SimSet.as_str(), "sim_set");
}

#[test]
fn every_kind_stored_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    for kind in ALL {
        store.plan_lease(t, kind, "k").unwrap();
    }
    let kinds: Vec<_> = store.leases(t).unwrap().iter().map(|r| r.kind).collect();
    assert_eq!(kinds, ALL);
}

#[test]
fn drop_removes_only_that_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    let one = store.plan_lease(a, LeaseKind::BoxDir, "/1").unwrap();
    let two = store.plan_lease(a, LeaseKind::Home, "/2").unwrap();
    let three = store.plan_lease(b, LeaseKind::BoxDir, "/3").unwrap();
    assert!(store.drop_lease(two).unwrap());
    let ids: Vec<_> = store.all_leases().unwrap().iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![one, three]);
    assert_eq!(store.leases(a).unwrap().len(), 1);
}

#[test]
fn new_lease_has_no_attempts_or_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    store.plan_lease(t, LeaseKind::BoxDir, "/b").unwrap();
    let row = &store.leases(t).unwrap()[0];
    assert_eq!(row.attempts, 0);
    assert_eq!(row.failure, None);
}

#[test]
fn record_attempt_counts_and_sets_or_clears_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::Compose, "proj").unwrap();
    store
        .record_attempt(id, Some("compose down: exit 1"))
        .unwrap();
    store
        .record_attempt(id, Some("compose down: timeout"))
        .unwrap();
    let row = &store.all_leases().unwrap()[0];
    assert_eq!(row.attempts, 2);
    assert_eq!(row.failure.as_deref(), Some("compose down: timeout"));
    store.record_attempt(id, None).unwrap();
    let row = &store.leases(t).unwrap()[0];
    assert_eq!(row.attempts, 3);
    assert_eq!(row.failure, None);
}

#[test]
fn record_attempt_bumps_updated_at_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::Home, "/h").unwrap();
    clock.sleep(std::time::Duration::from_millis(500));
    store.record_attempt(id, Some("busy")).unwrap();
    assert_eq!(
        store.leases(t).unwrap()[0].updated_at,
        Ts::from_millis(1_500)
    );
    drop(store);
    let row = open(dir.path()).leases(t).unwrap().remove(0);
    assert_eq!((row.attempts, row.failure.as_deref()), (1, Some("busy")));
}

#[test]
fn old_lease_table_gains_attempt_columns_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    drop(store);
    // The table as earlier builds created it: no `attempts`, no `failure`.
    let conn = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    conn.execute_batch(
        "DROP TABLE box_lease;
         CREATE TABLE box_lease (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           task INTEGER NOT NULL REFERENCES tasks(id),
           kind TEXT NOT NULL, key TEXT NOT NULL, state TEXT NOT NULL,
           booted INTEGER NOT NULL DEFAULT 0,
           pgid INTEGER, sid INTEGER, root_pid INTEGER, root_start INTEGER,
           size_at_release INTEGER,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
           UNIQUE(task, kind, key));",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO box_lease (task, kind, key, state, created_at, updated_at)
         VALUES (?, 'home', '/h', 'planned', 1, 1)",
        [t.get()],
    )
    .unwrap();
    drop(conn);
    let store = open(dir.path());
    let row = store.leases(t).unwrap().remove(0);
    assert_eq!((row.attempts, row.failure.as_deref()), (0, None));
    store.record_attempt(row.id, Some("busy")).unwrap();
    drop(store);
    let row = open(dir.path()).leases(t).unwrap().remove(0);
    assert_eq!((row.attempts, row.failure.as_deref()), (1, Some("busy")));
}

#[test]
fn retry_after_doubles_from_ten_minutes_and_caps_at_six_hours() {
    let min = |m: u64| Duration::from_secs(m * 60);
    assert_eq!(retry_after(0), min(10));
    assert_eq!(retry_after(1), min(10));
    assert_eq!(retry_after(2), min(20));
    assert_eq!(retry_after(3), min(40));
    assert_eq!(retry_after(6), min(320));
    assert_eq!(retry_after(7), min(360));
    assert_eq!(retry_after(8), min(360));
    assert_eq!(retry_after(u32::MAX), min(360));
}

#[test]
fn record_attempt_sets_then_clears_next_attempt_at() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::Home, "/h").unwrap();
    assert_eq!(store.leases(t).unwrap()[0].next_attempt_at, None);
    store.record_attempt(id, Some("busy")).unwrap();
    let first = Ts::from_millis(1_000).saturating_add(retry_after(1));
    assert_eq!(store.leases(t).unwrap()[0].next_attempt_at, Some(first));
    clock.sleep(Duration::from_secs(60));
    store.record_attempt(id, Some("busy")).unwrap();
    let second = Ts::from_millis(61_000).saturating_add(retry_after(2));
    assert_eq!(store.leases(t).unwrap()[0].next_attempt_at, Some(second));
    store.record_attempt(id, None).unwrap();
    assert_eq!(store.leases(t).unwrap()[0].next_attempt_at, None);
}

#[test]
fn due_when_unset_or_reached() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let id = store.plan_lease(t, LeaseKind::Home, "/h").unwrap();
    let mut row = store.leases(t).unwrap().remove(0);
    assert_eq!(row.id, id);
    assert!(row.due(Ts::from_millis(0)));
    row.next_attempt_at = Some(Ts::from_millis(500));
    assert!(!row.due(Ts::from_millis(499)));
    assert!(row.due(Ts::from_millis(500)));
    assert!(row.due(Ts::from_millis(501)));
}

#[test]
fn old_lease_table_gains_next_attempt_at_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    drop(store);
    let conn = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    conn.execute_batch(
        "DROP TABLE box_lease;
         CREATE TABLE box_lease (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           task INTEGER NOT NULL REFERENCES tasks(id),
           kind TEXT NOT NULL, key TEXT NOT NULL, state TEXT NOT NULL,
           booted INTEGER NOT NULL DEFAULT 0,
           pgid INTEGER, sid INTEGER, root_pid INTEGER, root_start INTEGER,
           size_at_release INTEGER,
           attempts INTEGER NOT NULL DEFAULT 0, failure TEXT,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
           UNIQUE(task, kind, key));",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO box_lease (task, kind, key, state, created_at, updated_at)
         VALUES (?, 'home', '/h', 'planned', 1, 1)",
        [t.get()],
    )
    .unwrap();
    drop(conn);
    let store = open(dir.path());
    let row = store.leases(t).unwrap().remove(0);
    assert_eq!(row.next_attempt_at, None);
    store.record_attempt(row.id, Some("busy")).unwrap();
    drop(store);
    let row = open(dir.path()).leases(t).unwrap().remove(0);
    assert_eq!(
        row.next_attempt_at,
        Some(Ts::from_millis(1_000).saturating_add(retry_after(1)))
    );
}
