//! Tests for `open`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::surface::SurfaceError;
use moor_core::{BoxId, BoxName, ErrorKind, ProviderId, TaskReq};

use super::{connect, run};
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    id: BoxId,
}

/// Task `t` with a live Box, working, placed in workspace `ws-9`.
fn fx_with(driver: FakeDriver) -> Fx {
    fx_placed(driver, Some("ws-9"))
}

fn fx_placed(driver: FakeDriver, workspace: Option<&str>) -> Fx {
    fx_surfaced(driver, FakeSurface::new(PathBuf::from("/s")), workspace)
}

fn fx_surfaced(driver: FakeDriver, surface: FakeSurface, workspace: Option<&str>) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let surface = Arc::new(surface);
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        surface.clone(),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
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
        workspace_id: workspace.map_or(Field::Keep, |w| Field::Set(w.to_owned())),
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
        surface,
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
        task: BoxName::parse_allow_bare("t").unwrap(),
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

fn focuses(f: &Fx) -> Vec<String> {
    let calls = f.surface.calls();
    calls
        .into_iter()
        .filter(|c| c.starts_with("focus_workspace"))
        .collect()
}

#[test]
fn awake_task_focuses_its_workspace() {
    let f = fx();
    let v = run(&f.cx, &req()).unwrap();
    assert!(!v.woke);
    assert_eq!(wakes(&f), 0);
    assert_eq!(focuses(&f), vec!["focus_workspace ws-9".to_owned()]);
    assert_eq!(states(&f), (TaskState::Working, BoxState::Ready));
}

#[test]
fn asleep_task_is_woken_then_focused() {
    let f = fx();
    asleep(&f);
    let v = run(&f.cx, &req()).unwrap();
    assert!(v.woke);
    assert_eq!(v.task, "t");
    assert_eq!(v.r#box.unwrap().state, BoxState::Ready);
    assert_eq!(wakes(&f), 1);
    assert_eq!(focuses(&f), vec!["focus_workspace ws-9".to_owned()]);
    assert_eq!(states(&f), (TaskState::Working, BoxState::Ready));
}

#[test]
fn second_open_focuses_again_without_waking() {
    let f = fx();
    asleep(&f);
    assert!(run(&f.cx, &req()).unwrap().woke);
    assert!(!run(&f.cx, &req()).unwrap().woke);
    assert_eq!(wakes(&f), 1);
    assert_eq!(focuses(&f).len(), 2);
}

#[test]
fn unknown_task_is_failed_and_not_focused() {
    let f = fx();
    let r = TaskReq {
        task: BoxName::parse_allow_bare("nope").unwrap(),
    };
    let err = run(&f.cx, &r).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(
        err.error.to_string().contains("no open task"),
        "{}",
        err.error
    );
    assert_eq!(wakes(&f), 0);
    assert!(focuses(&f).is_empty());
}

#[test]
fn failed_resume_leaves_box_asleep_and_unfocused() {
    let f = fx_with(FakeDriver::new().fail_wake(DriverError::Quota("q".into())));
    asleep(&f);
    run(&f.cx, &req()).unwrap_err();
    assert_eq!(states(&f), (TaskState::Asleep, BoxState::Asleep));
    assert!(focuses(&f).is_empty());
}

#[test]
fn gone_box_is_refused_without_a_resume_or_focus() {
    let f = fx();
    asleep(&f);
    let d = Decision::new(f.id).box_input(BoxInput::DriverMissing);
    f.cx.store().unwrap().apply(d).unwrap();
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(wakes(&f), 0);
    assert!(focuses(&f).is_empty());
}

#[test]
fn task_without_a_workspace_is_refused() {
    let f = fx_placed(FakeDriver::new(), None);
    let err = run(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(focuses(&f).is_empty());
}

#[test]
fn value_serializes_task_box_and_woke() {
    let f = fx();
    let v = run(&f.cx, &req()).unwrap();
    let j = serde_json::to_value(&v).unwrap();
    let obj = j.as_object().unwrap();
    assert_eq!(obj["task"], "t");
    assert_eq!(obj["woke"], false);
    assert!(obj.contains_key("box"));
}

#[test]
fn open_refuses_while_another_op_holds_the_box_lock() {
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
    assert!(focuses(&f).is_empty());
}

#[test]
fn failed_focus_after_a_wake_leaves_the_box_awake() {
    let surface = FakeSurface::new(PathBuf::from("/s")).fail_focus_workspace(SurfaceError::Timeout);
    let f = fx_surfaced(FakeDriver::new(), surface, Some("ws-9"));
    asleep(&f);
    run(&f.cx, &req()).unwrap_err();
    assert_eq!(wakes(&f), 1);
    assert_eq!(states(&f), (TaskState::Working, BoxState::Ready));
    let v = run(&f.cx, &req()).unwrap();
    assert!(!v.woke);
    assert_eq!(wakes(&f), 1);
    assert_eq!(focuses(&f).len(), 2);
}

#[test]
fn connect_wakes_and_returns_focus_command_without_focusing() {
    let f = fx();
    asleep(&f);
    let v = connect(&f.cx, &req()).unwrap();
    assert!(v.woke);
    assert_eq!(wakes(&f), 1);
    assert!(focuses(&f).is_empty());
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        serde_json::json!({"tool":"herdr","args":["workspace","focus","ws-9"],"woke":true})
    );
}

#[test]
fn connect_without_a_workspace_is_refused() {
    let f = fx_placed(FakeDriver::new(), None);
    let err = connect(&f.cx, &req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn connect_unknown_task_is_failed() {
    let f = fx();
    let r = TaskReq {
        task: BoxName::parse_allow_bare("nope").unwrap(),
    };
    let err = connect(&f.cx, &r).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("no open task"));
}
