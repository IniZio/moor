use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{Acquired, DriverError, Platform};
use moor_core::{BoxHandle, BoxPath, TaskId, TaskName};

use super::{req, world};
use crate::ops::start::run;

use crate::ops::common::provision::hold_root;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// Plans a `BoxRoot` row under "planned-1", holds the root for `got`, and returns the
/// state of the row under `key`.
fn state_after_hold(got: &str, key: &str) -> Option<LeaseState> {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("moor.db"),
        Arc::new(FakeClock::new(Ts::from_millis(1_000))),
    )
    .unwrap();
    let id: TaskId = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let planned = BoxHandle::new("planned-1");
    let root = store
        .plan_lease(id, LeaseKind::BoxRoot, planned.as_str())
        .unwrap();
    let got = Acquired {
        handle: BoxHandle::new(got),
        workdir: BoxPath::restore("/work").unwrap(),
        platform: Platform::Linux,
    };
    hold_root(&store, id, root, &planned, &got).unwrap();
    store
        .leases(id)
        .unwrap()
        .iter()
        .find(|r| r.kind == LeaseKind::BoxRoot && r.key == key)
        .map(|r| r.state)
}

#[test]
fn an_acquired_handle_off_the_plan_is_held_and_the_planned_row_stays_planned() {
    assert_eq!(
        state_after_hold("other-1", "planned-1"),
        Some(LeaseState::Planned)
    );
    assert_eq!(
        state_after_hold("other-1", "other-1"),
        Some(LeaseState::Held)
    );
}

#[test]
fn an_acquired_handle_on_the_plan_holds_the_planned_row() {
    assert_eq!(
        state_after_hold("planned-1", "planned-1"),
        Some(LeaseState::Held)
    );
}

fn boom() -> DriverError {
    DriverError::Failed {
        summary: "boom".into(),
    }
}

#[test]
fn a_failed_acquire_releases_the_attempted_handle_and_drops_the_root() {
    let w = world(FakeDriver::new().fail_acquire(boom()));
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert!(err.error.to_string().contains("boom"), "{err:?}");
    assert!(w.driver.calls().contains(&"release box-1".to_string()));
    let store = w.cx.store().unwrap();
    let name = TaskName::parse("demo").unwrap();
    let task = store.open_task_by_name(&name).unwrap().unwrap();
    assert!(store.leases(task.id).unwrap().is_empty());
}

#[test]
fn a_failed_release_keeps_the_root_and_the_start_error() {
    let w = world(
        FakeDriver::new()
            .fail_acquire(boom())
            .fail_release(DriverError::Failed {
                summary: "stop broke".into(),
            }),
    );
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert!(err.error.to_string().contains("boom"), "{err:?}");
    let store = w.cx.store().unwrap();
    let name = TaskName::parse("demo").unwrap();
    let task = store.open_task_by_name(&name).unwrap().unwrap();
    let rows = store.leases(task.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().unwrap().kind, LeaseKind::BoxRoot);
}
