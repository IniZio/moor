//! Tests for name lookups and live-box scans.

use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::{ProviderId, TaskId, TaskName};

use crate::testing::clock::FakeClock;

use super::Store;

/// Open a store in a temp dir.
fn open(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
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

/// Close a task.
fn close(store: &Store, id: TaskId) {
    store
        .apply(Decision::new(id).task_input(TaskInput::Close))
        .unwrap();
}

#[test]
fn name_lookups_distinguish_open_from_latest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let name = TaskName::parse("t").unwrap();
    assert!(store.open_task_by_name(&name).unwrap().is_none());
    assert!(store.latest_task_by_name(&name).unwrap().is_none());

    let first = add(&store, "t");
    assert_eq!(store.open_task_by_name(&name).unwrap().unwrap().id, first);
    close(&store, first);
    assert!(store.open_task_by_name(&name).unwrap().is_none());
    let latest = store.latest_task_by_name(&name).unwrap().unwrap();
    assert_eq!((latest.id, latest.state), (first, TaskState::Closed));

    let second = add(&store, "t");
    assert_eq!(store.open_task_by_name(&name).unwrap().unwrap().id, second);
    assert_eq!(
        store.latest_task_by_name(&name).unwrap().unwrap().id,
        second
    );
}

#[test]
fn all_tasks_lists_closed_too_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    close(&store, a);
    let ids: Vec<_> = store
        .all_tasks()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, [a, b]);
}

#[test]
fn box_rows_live_skips_deleted_boxes() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    let live = add(&store, "live");
    let gone = add(&store, "gone");
    let _none = add(&store, "none");
    for id in [live, gone] {
        let d = Decision::new(id)
            .task_input(TaskInput::Admitted)
            .box_input(BoxInput::Requested)
            .set(fields.clone());
        store.apply(d).unwrap();
    }
    let failed = BoxInput::AcquireFailed {
        attempted: None,
        cause: "x".into(),
    };
    store.apply(Decision::new(gone).box_input(failed)).unwrap();
    let rows = store.box_rows_live().unwrap();
    let got: Vec<_> = rows.iter().map(|(id, b)| (*id, b.state)).collect();
    assert_eq!(got, [(live, BoxState::Starting)]);
}
