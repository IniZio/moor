//! Tests for versioned Report storage.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::{TaskId, TaskName};

use crate::store::Store;
use crate::testing::clock::FakeClock;

/// Open a store in `dir` with a fake clock.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

#[test]
fn first_add_is_version_one_with_content_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let reports = dir.path().join("reports");
    let row = store
        .add_report(t, "plan", "hello", &reports)
        .unwrap()
        .unwrap();
    assert_eq!(row.version, 1);
    assert_eq!(row.created_at, Ts::from_millis(1_000));
    assert_eq!(row.title, None);
    assert_eq!(fs::read_to_string(&row.path).unwrap(), "hello");
    assert_eq!(
        Path::new(&row.path),
        reports.join(t.get().to_string()).join("plan.v1.md")
    );
    assert_eq!(store.reports(t).unwrap(), vec![row]);
}

#[test]
fn same_content_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    store
        .add_report(t, "plan", "x", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(store.add_report(t, "plan", "x", dir.path()).unwrap(), None);
    assert_eq!(store.reports(t).unwrap().len(), 1);
}

#[test]
fn changed_content_appends_a_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let v1 = store
        .add_report(t, "plan", "one", dir.path())
        .unwrap()
        .unwrap();
    let v2 = store
        .add_report(t, "plan", "two", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(v2.version, 2);
    assert_ne!(v1.path, v2.path);
    assert_eq!(fs::read_to_string(&v1.path).unwrap(), "one");
    assert_eq!(fs::read_to_string(&v2.path).unwrap(), "two");
}

#[test]
fn kinds_version_independently_and_list_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    store.add_report(t, "summary", "s1", dir.path()).unwrap();
    store.add_report(t, "plan", "p1", dir.path()).unwrap();
    store.add_report(t, "summary", "s2", dir.path()).unwrap();
    store.add_report(t, "plan", "p2", dir.path()).unwrap();
    let got: Vec<(String, i64)> = store
        .reports(t)
        .unwrap()
        .into_iter()
        .map(|r| (r.kind, r.version))
        .collect();
    let want =
        [("plan", 1), ("plan", 2), ("summary", 1), ("summary", 2)].map(|(k, v)| (k.to_owned(), v));
    assert_eq!(got, want);
}

#[test]
fn bad_kind_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    for kind in ["", ".", "..", "a/b"] {
        assert!(store.add_report(t, kind, "x", dir.path()).is_err());
    }
    assert!(store.reports(t).unwrap().is_empty());
}
