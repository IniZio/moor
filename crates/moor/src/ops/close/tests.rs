use std::path::PathBuf;
use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan};
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::SurfaceError;
use moor_core::{CloseReq, ErrorKind, ProviderId, TaskId, TaskName};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

mod agent;
mod promote;
mod teardown;

const RAW: &str = "RAWDRIVER-7731";

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    id: TaskId,
}

fn req(force: bool) -> CloseReq {
    CloseReq {
        task: TaskName::parse("t").unwrap(),
        force,
    }
}

/// Git answers: `unpushed` commits and a clean tree.
fn proc(unpushed: u32) -> Arc<ScriptedProc> {
    let p = ScriptedProc::new();
    p.on(Tool::Git, &["rev-list"], Reply::Ok(format!("{unpushed}\n")));
    p.on(Tool::Git, &["status"], Reply::Ok(String::new()));
    p.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/main/.git\n".to_owned()),
    );
    Arc::new(p)
}

/// A Ready task with a Box from `driver`; the worktree is a real directory.
fn fx_with(driver: FakeDriver, surface: FakeSurface, unpushed: u32) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let surface = Arc::new(surface);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc(unpushed),
    );
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("t").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set(dir.path().display().to_string()),
        workspace_id: Field::Set("ws-1".into()),
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
            name: TaskName::parse("t").unwrap(),
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
        dir,
        cx,
        driver,
        surface,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new(), FakeSurface::new(PathBuf::from("/s")), 0)
}

fn row(f: &Fx) -> crate::store::TaskRow {
    f.cx.store().unwrap().task(f.id).unwrap().unwrap()
}

fn box_state(f: &Fx) -> BoxState {
    row(f).boxed.unwrap().state
}

#[test]
fn clean_close_deletes_the_box_and_closes() {
    let f = fx();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.box_deleted && v.workspace_closed && v.worktree_kept);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("release")));
    assert!(f.dir.path().is_dir());
}

#[test]
fn unpushed_work_is_refused_without_force() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 2);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(row(&f).state, TaskState::Working);
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("release")));
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
}

#[test]
fn release_not_found_counts_as_deleted() {
    let f = fx();
    let h = row(&f).boxed.unwrap().handle.unwrap();
    f.driver.forget(&h);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(box_state(&f), BoxState::Deleted);
}

#[test]
fn release_error_on_a_ready_box_keeps_the_task_open() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        FakeSurface::new("/s"),
        0,
    );
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(
        e.error.to_string().contains(RAW),
        "the adapter summary is surfaced"
    );
    assert_eq!(row(&f).state, TaskState::Working);
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn release_error_on_a_failed_box_warns_and_closes() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        FakeSurface::new("/s"),
        0,
    );
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() }))
        .unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(!v.box_deleted);
    assert_eq!(box_state(&f), BoxState::ReleaseFailed);
    let warnings = f.cx.take_warnings();
    assert!(!warnings.is_empty());
    assert!(warnings.iter().all(|w| !w.message.contains(RAW)));
}

#[test]
fn missing_workspace_is_tolerated() {
    let f = fx_with(
        FakeDriver::new(),
        FakeSurface::new("/s").fail_close_workspace(SurfaceError::NotFound),
        0,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.workspace_closed);
    assert!(f.cx.take_warnings().is_empty());
}
