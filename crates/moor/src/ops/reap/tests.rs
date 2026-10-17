//! The reaper releases orphan leases of closed and rejected Tasks, leaves open Tasks alone, drops rows of a Task that never had a Box and holds a Task back until every row's backoff has passed.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::{ProviderId, TaskId, TaskName};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::{BoxLock, WAIT};
use crate::ops::common::release::testkit::framed;
use crate::store::lease::LeaseKind;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    clock: Arc<FakeClock>,
    driver: Arc<FakeDriver>,
}

/// A driver whose release scripts print nothing of their own; `failing` exec results come first.
fn driver(failing: usize) -> FakeDriver {
    let release: Vec<String> = (0..16).map(|i| framed(i, "")).collect();
    FakeDriver::new()
        .run_stdouts(release.iter().map(String::as_str))
        .exec_results((0..failing).map(|_| Ok(ExecOutcome::Exited { code: 1 })))
}

fn fx(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let cx = Cx::fake_with_clock(
        dir.path(),
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new("/s")),
        Arc::new(ScriptedProc::new()),
        crate::testing::cx::fake_config(),
        clock.clone(),
    );
    Fx {
        _dir: dir,
        cx,
        clock,
        driver,
    }
}

/// Admits Task `name` with a Ready Box and one compose lease row.
fn task(f: &Fx, name: &str) -> TaskId {
    let store = f.cx.store().unwrap();
    let name = TaskName::parse(name).unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        ..Fields::default()
    };
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(fields),
        )
        .unwrap();
    let got = f
        .driver
        .acquire(&AcquireReq {
            provider,
            task: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::BoxReady {
                    setup: SetupOutcome::Ok,
                })
                .box_input(BoxInput::Acquired { handle: got.handle }),
        )
        .unwrap();
    store.plan_lease(id, LeaseKind::Compose, "proj").unwrap();
    id
}

fn end(f: &Fx, id: TaskId, input: TaskInput) {
    f.cx.store()
        .unwrap()
        .apply(Decision::new(id).task_input(input))
        .unwrap();
}

fn rows(f: &Fx, id: TaskId) -> usize {
    f.cx.store().unwrap().leases(id).unwrap().len()
}

fn released(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("release"))
        .count()
}

#[test]
fn a_closed_task_has_its_leases_released_and_dropped() {
    let f = fx(driver(0));
    let id = task(&f, "a");
    end(&f, id, TaskInput::Close);
    let lines = run(&f.cx).unwrap();
    assert_eq!(rows(&f, id), 0);
    assert_eq!(released(&f), 1);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("released"), "{lines:?}");
}

#[test]
fn a_rejected_task_with_no_box_has_its_rows_dropped_and_says_so_once() {
    let f = fx(driver(0));
    let store = f.cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("a").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    store.plan_lease(id, LeaseKind::Compose, "proj").unwrap();
    end(
        &f,
        id,
        TaskInput::Rejected {
            reason: "no".to_owned(),
        },
    );
    let state = store.task(id).unwrap().unwrap().state;
    assert_eq!(state, TaskState::Rejected);
    let lines = run(&f.cx).unwrap();
    assert_eq!(rows(&f, id), 0);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("no Box ever existed"), "{lines:?}");
    assert_eq!(released(&f), 0);
    assert!(run(&f.cx).unwrap().is_empty());
}

#[test]
fn an_open_task_is_never_touched_even_with_failed_rows() {
    let f = fx(driver(0));
    let id = task(&f, "a");
    let lease = f.cx.store().unwrap().leases(id).unwrap()[0].id;
    f.cx.store()
        .unwrap()
        .record_attempt(lease, Some("boom"))
        .unwrap();
    f.clock.sleep(Duration::from_hours(24));
    assert!(run(&f.cx).unwrap().is_empty());
    assert_eq!(rows(&f, id), 1);
    assert_eq!(released(&f), 0);
}

#[test]
fn a_driver_failure_backs_rows_off_then_the_task_is_retried() {
    let f = fx(driver(0).fail_release(DriverError::Failed {
        summary: "boom".into(),
    }));
    let id = task(&f, "a");
    end(&f, id, TaskInput::Close);
    let first = run(&f.cx).unwrap();
    assert!(first[0].contains("will retry"), "{first:?}");
    assert_eq!(rows(&f, id), 1);

    assert!(run(&f.cx).unwrap().is_empty(), "not yet due");
    assert_eq!(released(&f), 1);

    f.clock.sleep(Duration::from_mins(11));
    let again = run(&f.cx).unwrap();
    assert!(again[0].contains("released"), "{again:?}");
    assert_eq!(rows(&f, id), 0);
    assert_eq!(released(&f), 2);
}

#[test]
fn two_runs_inside_the_backoff_release_the_box_once() {
    let f = fx(driver(0).fail_release_always(DriverError::Failed {
        summary: "boom".into(),
    }));
    let id = task(&f, "a");
    end(&f, id, TaskInput::Close);
    run(&f.cx).unwrap();
    f.clock.sleep(Duration::from_mins(1));
    assert!(run(&f.cx).unwrap().is_empty());
    assert_eq!(released(&f), 1);
    assert_eq!(rows(&f, id), 1);
}

#[test]
fn a_failed_row_holds_back_the_whole_task_until_its_backoff_passes() {
    let f = fx(driver(0));
    let id = task(&f, "a");
    let store = f.cx.store().unwrap();
    store.plan_lease(id, LeaseKind::Compose, "other").unwrap();
    let failed = store.leases(id).unwrap()[0].id;
    store.record_attempt(failed, Some("boom")).unwrap();
    end(&f, id, TaskInput::Close);

    assert!(run(&f.cx).unwrap().is_empty());
    assert_eq!(released(&f), 0);
    assert_eq!(rows(&f, id), 2);

    f.clock.sleep(Duration::from_mins(11));
    let lines = run(&f.cx).unwrap();
    assert_eq!(released(&f), 1);
    assert!(lines[0].contains("released"), "{lines:?}");
}

#[test]
fn a_task_whose_box_lock_is_held_is_skipped_with_its_rows_kept() {
    let f = fx(driver(0));
    let id = task(&f, "a");
    end(&f, id, TaskInput::Close);
    let _held =
        BoxLock::acquire(f.cx.clock(), f.cx.state_dir().unwrap(), id, "close", WAIT).unwrap();
    let lines = run(&f.cx).unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("skipped"), "{lines:?}");
    assert_eq!(rows(&f, id), 1);
    assert_eq!(released(&f), 0);
}

#[test]
fn one_failing_task_does_not_block_another() {
    let f = fx(driver(0).fail_release(DriverError::Failed {
        summary: "boom".into(),
    }));
    let bad = task(&f, "a");
    let good = task(&f, "b");
    end(&f, bad, TaskInput::Close);
    end(&f, good, TaskInput::Close);
    let lines = run(&f.cx).unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(rows(&f, bad), 1);
    assert!(f.cx.store().unwrap().leases(bad).unwrap()[0]
        .failure
        .is_some());
    assert_eq!(rows(&f, good), 0);
    assert_eq!(released(&f), 2);
}
