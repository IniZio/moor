use std::sync::{Arc, Mutex};

use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use moor_core::{BoxId, BoxName};

use super::setup::{exit, start_req};
use super::{req, world, world_at};
use crate::ops::start::run;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

#[test]
fn a_started_box_leaves_its_root_and_workdir_held() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    let rows =
        w.cx.store()
            .unwrap()
            .leases(BoxId::new(v.boxed.id))
            .unwrap();
    let state = |kind| {
        rows.iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.state)
            .collect::<Vec<_>>()
    };
    assert_eq!(state(LeaseKind::BoxRoot), vec![LeaseState::Held]);
    assert_eq!(state(LeaseKind::BoxDir), vec![LeaseState::Held]);
}

#[test]
fn a_failed_acquire_leaves_the_root_planned() {
    let w = world(FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    }));
    run(&w.cx, &req("demo")).unwrap_err();
    let store = w.cx.store().unwrap();
    let task = store
        .open_task_by_name(&BoxName::parse_allow_bare("demo").unwrap())
        .unwrap()
        .unwrap();
    let rows = store.leases(task.id).unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows.first().unwrap();
    assert_eq!(row.kind, LeaseKind::BoxRoot);
    assert_eq!(row.state, LeaseState::Planned);
}

#[test]
fn leases_survive_reopening_the_store() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    let id = BoxId::new(v.boxed.id);
    let before = w.cx.store().unwrap().leases(id).unwrap();
    let path = w.cx.state_dir().unwrap().join(STORE_FILE);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let reopened = Store::open(&path, clock).unwrap();
    let after = reopened.leases(id).unwrap();
    assert!(!after.is_empty());
    assert_eq!(after.len(), before.len());
    assert!(after.iter().all(|r| r.state == LeaseState::Held));
}

#[test]
fn a_failed_setup_leaves_the_workdir_held() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let w = world_at(driver, repo.path());
    // Read the workdir lease from a second connection while each exec runs, so a
    // crash mid-setup would find it already held.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, path) = (Arc::clone(&seen), w.dir.path().join(STORE_FILE));
    w.driver.on_exec(move || {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        let store = Store::open(&path, clock).unwrap();
        let rows = store.leases(BoxId::new(1)).unwrap();
        let dir: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == LeaseKind::BoxDir)
            .map(|r| r.state)
            .collect();
        sink.lock().unwrap().push(dir);
    });
    let v = run(&w.cx, &start_req()).unwrap();
    assert!(!v.setup.ok);
    assert_eq!(*seen.lock().unwrap(), vec![vec![LeaseState::Held]; 3]);
    let rows =
        w.cx.store()
            .unwrap()
            .leases(BoxId::new(v.boxed.id))
            .unwrap();
    let dir: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == LeaseKind::BoxDir)
        .map(|r| r.state)
        .collect();
    assert_eq!(dir, vec![LeaseState::Held]);
}
