//! Tests for `wake_if_asleep`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan};
use moor_core::{ErrorKind, ProviderId, TaskId, TaskName};

use super::wake_if_asleep;
use crate::cx::Cx;
use crate::store::TaskRow;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: TaskId,
}

/// Task `t` Working with a Ready Box.
fn fx_with(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = TaskName::parse("t").unwrap();
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
        worktree: Field::Set("/home/me/wt/t".to_owned()),
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
    let got = driver
        .acquire(&AcquireReq {
            provider,
            task: id,
            name,
            egress: EgressPlan::Open,
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
    Fx {
        _dir: dir,
        cx,
        driver,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new())
}

/// Put the Task and Box to sleep.
fn sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(f.id)
                .task_input(TaskInput::SleepCmd)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
}

fn row(f: &Fx) -> TaskRow {
    let name = TaskName::parse("t").unwrap();
    f.cx.store()
        .unwrap()
        .open_task_by_name(&name)
        .unwrap()
        .unwrap()
}

fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

fn wake_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("wake"))
        .count()
}

#[test]
fn asleep_task_and_box_are_woken() {
    let f = fx();
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert!(f.driver.calls().contains(&"wake box-1".to_owned()));
    assert_eq!(wake_calls(&f), 1);
    let r = row(&f);
    assert_eq!(r.state, TaskState::Working);
    assert_eq!(r.boxed.unwrap().state, BoxState::Ready);
    assert!(kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn awake_task_is_left_alone() {
    let f = fx();
    let store = f.cx.store().unwrap();
    assert!(!wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(wake_calls(&f), 0);
    assert_eq!(row(&f).state, TaskState::Working);
}

#[test]
fn failed_resume_leaves_task_and_box_asleep() {
    let f = fx_with(FakeDriver::new().fail_wake(DriverError::NotFound));
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).is_err());
    let r = row(&f);
    assert_eq!(r.state, TaskState::Asleep);
    assert_eq!(r.boxed.unwrap().state, BoxState::Asleep);
    assert!(!kinds(&f).contains(&"woke".to_owned()));
}

#[test]
fn gone_box_is_refused_without_a_driver_call() {
    let f = fx();
    sleep(&f);
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let err = wake_if_asleep(&f.cx, store, &row(&f)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(wake_calls(&f), 0);
}

#[test]
fn asleep_task_with_ready_box_wakes_only_the_task() {
    let f = fx();
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::SleepCmd))
        .unwrap();
    assert_eq!(row(&f).state, TaskState::Asleep);
    let ready_before = kinds(&f).iter().filter(|k| *k == "box_ready").count();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(wake_calls(&f), 0);
    let r = row(&f);
    assert_eq!(r.state, TaskState::Working);
    assert_eq!(r.boxed.unwrap().state, BoxState::Ready);
    let after = kinds(&f);
    assert!(after.contains(&"woke".to_owned()));
    assert_eq!(
        after.iter().filter(|k| *k == "box_ready").count(),
        ready_before
    );
}

#[test]
fn asleep_box_with_working_task_wakes_only_the_box() {
    let f = fx();
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::Sleep))
        .unwrap();
    assert_eq!(row(&f).boxed.unwrap().state, BoxState::Asleep);
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(wake_calls(&f), 1);
    let r = row(&f);
    assert_eq!(r.state, TaskState::Working);
    assert_eq!(r.boxed.unwrap().state, BoxState::Ready);
    assert!(!kinds(&f).contains(&"woke".to_owned()));
}
