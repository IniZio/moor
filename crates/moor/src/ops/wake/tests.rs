//! Tests for `wake`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, SleepKind};
use moor_core::{ErrorKind, ProviderId, TaskId, TaskName, TaskReq};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: TaskId,
}

/// Task `t` with a live Box, working.
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
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            task: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
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

/// The fixture put to sleep: Task Asleep and Box Asleep.
fn asleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    let d = Decision::new(f.id)
        .task_input(TaskInput::SleepCmd)
        .box_input(BoxInput::Sleep);
    store.apply(d).unwrap();
}

fn req() -> TaskReq {
    TaskReq {
        task: TaskName::parse("t").unwrap(),
    }
}

fn states(f: &Fx) -> (TaskState, BoxState) {
    let row = f.cx.store().unwrap().task(f.id).unwrap().unwrap();
    (row.state, row.boxed.unwrap().state)
}

fn wakes(f: &Fx) -> usize {
    let calls = f.driver.calls();
    calls.iter().filter(|c| c.starts_with("wake")).count()
}

#[test]
fn asleep_task_wakes_its_box() {
    let f = fx();
    asleep(&f);
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
    let v = run(&f.cx, &req()).unwrap();
    assert!(v.woke);
    assert_eq!(v.task, "t");
    assert_eq!(v.r#box.unwrap().state, BoxState::Ready);
    assert!(f.driver.calls().contains(&"wake box-1".to_owned()));
    assert_eq!(wakes(&f), 1);
    assert_eq!(states(&f), (TaskState::Working, BoxState::Ready));
}

#[test]
fn task_that_is_not_asleep_is_left_alone() {
    let f = fx();
    let v = run(&f.cx, &req()).unwrap();
    assert!(!v.woke);
    assert_eq!(wakes(&f), 0);
    assert_eq!(states(&f), (TaskState::Working, BoxState::Ready));
}

#[test]
fn second_wake_does_nothing() {
    let f = fx();
    asleep(&f);
    assert!(run(&f.cx, &req()).unwrap().woke);
    assert!(!run(&f.cx, &req()).unwrap().woke);
    assert_eq!(wakes(&f), 1);
}

#[test]
fn failed_resume_leaves_task_and_box_asleep() {
    let f = fx_with(FakeDriver::new().fail_wake(DriverError::Quota("q".into())));
    asleep(&f);
    run(&f.cx, &req()).unwrap_err();
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
}

#[test]
fn gone_box_is_refused_without_a_resume() {
    let f = fx();
    asleep(&f);
    let d = Decision::new(f.id).box_input(BoxInput::DriverMissing);
    f.cx.store().unwrap().apply(d).unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(wakes(&f), 0);
    assert_eq!(states(&f).0, TaskState::Asleep);
}

#[test]
fn unknown_task_is_failed() {
    let f = fx();
    let r = TaskReq {
        task: TaskName::parse("nope").unwrap(),
    };
    let err = run(&f.cx, &r).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(wakes(&f), 0);
}

#[test]
fn wake_refuses_while_another_op_holds_the_box_lock() {
    let f = fx();
    asleep(&f);
    let _held = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("box busy"), "{}", err.error);
    assert_eq!(wakes(&f), 0);
}

#[test]
fn sleep_label_follows_provider_sleep_support() {
    for (kind, after) in [(SleepKind::None, None), (SleepKind::Suspend, Some("30m"))] {
        let f = fx_with(FakeDriver::new().sleep_kind(kind));
        asleep(&f);
        let v = run(&f.cx, &req()).unwrap();
        assert_eq!(v.r#box.unwrap().sleep.after.as_deref(), after);
    }
}
