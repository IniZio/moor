//! Tests for the Box lease records.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};

use super::{LeaseKind, LeaseState, ProcessIds};
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

fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
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
    let err = store.plan_lease(BoxId::new(999), LeaseKind::BoxDir, "/x");
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
