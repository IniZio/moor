//! Tests for name lookups and live-box scans.

use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use crate::testing::clock::FakeClock;

use super::{live_holders_in, Store};

/// Open a store in a temp dir.
fn open(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// Close a task.
fn close(store: &Store, id: BoxId) {
    store
        .apply(Decision::new(id).task_input(TaskInput::Close))
        .unwrap();
}

#[test]
fn name_lookups_distinguish_open_from_latest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let name = BoxName::parse("t").unwrap();
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

/// Request a box for `id` on `provider`.
fn request(store: &Store, id: BoxId, provider: &str) {
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new(provider)),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
}

fn box_input(store: &Store, id: BoxId, input: BoxInput) {
    store.apply(Decision::new(id).box_input(input)).unwrap();
}

fn acquired() -> BoxInput {
    BoxInput::Acquired {
        handle: BoxHandle::new("h"),
    }
}

#[test]
fn waiting_tasks_returns_waiting_for_box_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let d = add(&store, "d");
    let b = add(&store, "b");
    add(&store, "c");
    let a = add(&store, "a");
    assert!(store.waiting_tasks().unwrap().is_empty());
    // An index on (state, name) lets the planner return rows in name order.
    store
        .writer()
        .unwrap()
        .execute_batch("CREATE INDEX tasks_state_name ON tasks(state, name)")
        .unwrap();
    for id in [d, b, a] {
        let input = TaskInput::AtCapacity { max_boxes: 1 };
        store.apply(Decision::new(id).task_input(input)).unwrap();
    }
    let waiting = store.waiting_tasks().unwrap();
    assert_eq!(waiting.iter().map(|t| t.id).collect::<Vec<_>>(), [d, b, a]);
    assert!(waiting.iter().all(|t| t.state == TaskState::WaitingForBox));
}

#[test]
fn live_holders_lists_only_the_providers_live_boxes_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let p = ProviderId::new("p");
    assert!(live_holders_in(&store.reader().unwrap(), &p)
        .unwrap()
        .is_empty());

    let [starting, ready, failed, deleted, other] =
        ["e", "b", "c", "d", "a"].map(|n| add(&store, n));
    for id in [starting, ready, failed, deleted] {
        request(&store, id, "p");
    }
    request(&store, other, "q");
    for id in [ready, deleted] {
        box_input(&store, id, acquired());
    }
    box_input(&store, deleted, BoxInput::Released);
    let cause = "x".into();
    let fail = BoxInput::AcquireFailed {
        attempted: None,
        cause,
    };
    box_input(&store, failed, fail);

    let holders = live_holders_in(&store.reader().unwrap(), &p).unwrap();
    assert_eq!(
        holders.iter().map(|t| t.id).collect::<Vec<_>>(),
        [starting, ready]
    );
    assert!(holders.iter().all(|t| t.boxed.is_some()));
}
