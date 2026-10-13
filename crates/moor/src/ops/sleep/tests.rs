//! Tests for `sleep`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, EgressPlan, SleepOutcome};
use moor_core::{ErrorKind, ProviderId, TaskId, TaskName, TaskReq};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::view::task_view;
use crate::store::NewRun;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: TaskId,
}

/// Task `t` with a live Box, so the Task is Working.
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
            egress: EgressPlan::Open,
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

/// A fixture whose Task is Idle.
fn idle() -> Fx {
    idle_with(FakeDriver::new())
}

fn idle_with(driver: FakeDriver) -> Fx {
    let f = fx_with(driver);
    let d = Decision::new(f.id).task_input(TaskInput::AgentQuiet);
    f.cx.store().unwrap().apply(d).unwrap();
    f
}

fn req() -> TaskReq {
    TaskReq {
        task: TaskName::parse("t").unwrap(),
    }
}

/// Kinds of the events recorded for the task.
fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

/// Task and Box states now.
fn states(f: &Fx) -> (TaskState, BoxState) {
    let row = f.cx.store().unwrap().task(f.id).unwrap().unwrap();
    (row.state, row.boxed.unwrap().state)
}

/// The stored sleep reason of the Box.
fn reason(f: &Fx) -> Option<String> {
    let row = f.cx.store().unwrap().task(f.id).unwrap().unwrap();
    row.boxed.unwrap().sleep_reason
}

fn sleep_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("sleep "))
        .count()
}

/// Begin a run owned by a live pid.
fn begin_run(f: &Fx) {
    f.cx.store()
        .unwrap()
        .begin_run(NewRun {
            task: f.id,
            argv: vec!["make".into()],
            record: None,
            sha: None,
            sync: "none".into(),
            pid: 4242,
        })
        .unwrap();
}

#[test]
fn sleeps_an_idle_task() {
    let f = idle();
    let v = run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
    let k = kinds(&f);
    assert!(k.contains(&"asleep".to_owned()), "{k:?}");
    assert!(k.contains(&"box_asleep".to_owned()), "{k:?}");
    assert_eq!(v.task, "t");
    assert!(!v.woke);
    assert_eq!(v.r#box.unwrap().state, BoxState::Asleep);
    assert_eq!(sleep_calls(&f), 1);
}

#[test]
fn sleeps_a_working_task_without_a_run() {
    let f = fx();
    assert_eq!(states(&f).0, TaskState::Working);
    run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
}

#[test]
fn refuses_while_a_run_is_in_flight() {
    let f = fx();
    begin_run(&f);
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("run is in flight"),
        "{}",
        err.error
    );
    assert_eq!(sleep_calls(&f), 0);
    assert_eq!(states(&f), (TaskState::Working, BoxState::Ready));
}

#[test]
fn native_sleep_transitions_like_slept() {
    let f = idle_with(FakeDriver::new().sleep_outcome(SleepOutcome::Native));
    run(&f.cx, &req()).unwrap();
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
}

#[test]
fn cannot_sleep_refuses_and_stores_the_reason() {
    let f = idle_with(
        FakeDriver::new().sleep_outcome(SleepOutcome::CannotSleep("provider cannot wake".into())),
    );
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("provider cannot wake"),
        "{}",
        err.error
    );
    assert_eq!(states(&f), (TaskState::Idle, BoxState::Ready));
    assert_eq!(reason(&f).as_deref(), Some("provider cannot wake"));
    let row = f.cx.store().unwrap().task(f.id).unwrap().unwrap();
    let shown = task_view(&row).r#box.unwrap().sleep.reason;
    assert_eq!(shown.as_deref(), Some("provider cannot wake"));
    assert!(!kinds(&f).contains(&"asleep".to_owned()));
}

#[test]
fn a_later_successful_sleep_clears_the_reason() {
    let f = idle();
    let d = Decision::new(f.id).set(Fields {
        box_sleep_reason: Field::Set("old".to_owned()),
        ..Fields::default()
    });
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(reason(&f).as_deref(), Some("old"));
    run(&f.cx, &req()).unwrap();
    assert_eq!(reason(&f), None);
}

#[test]
fn unknown_task_fails() {
    let f = fx();
    let other = TaskReq {
        task: TaskName::parse("nope").unwrap(),
    };
    let err = run(&f.cx, &other).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(
        err.error.to_string().contains("no open task nope"),
        "{}",
        err.error
    );
}

#[test]
fn refuses_when_the_box_lock_is_held() {
    let f = idle();
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
    assert!(err.error.to_string().contains("busy"), "{}", err.error);
    assert_eq!(sleep_calls(&f), 0);
}

#[test]
fn an_asleep_task_is_left_alone() {
    let f = idle();
    run(&f.cx, &req()).unwrap();
    let v = run(&f.cx, &req()).unwrap();
    assert!(!v.woke);
    assert_eq!(sleep_calls(&f), 1);
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
}
