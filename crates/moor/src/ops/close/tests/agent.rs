use std::path::Path;

use moor_core::domain::boxes::BoxState;
use moor_core::domain::task::TaskState;
use moor_core::seam::driver::DriverError;
use moor_core::seam::surface::{PaneCmd, Surface, SurfaceError};
use moor_core::{ErrorKind, TaskName};

use super::{box_state, fx_with, req, row, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

fn live(f: &Fx, pane: &str) {
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some(pane))
        .unwrap();
}

fn open_pane(f: &Fx) -> String {
    let cmd = PaneCmd {
        label: "agent".into(),
        argv: vec!["sh".into()],
        cwd: None,
        env: Vec::new(),
    };
    let wt = f
        .surface
        .create_worktree(Path::new("r"), &TaskName::parse("t").unwrap(), None)
        .unwrap();
    let ws = f.surface.ensure_workspace(&wt, "t").unwrap();
    let pane = f.surface.open_pane(&ws, &cmd).unwrap();
    pane.as_str().to_owned()
}

fn live_session(f: &Fx) -> bool {
    f.cx.store().unwrap().live_session(f.id).unwrap().is_some()
}

fn index(calls: &[String], prefix: &str) -> usize {
    calls.iter().position(|c| c.starts_with(prefix)).unwrap()
}

#[test]
fn close_stops_the_agent_before_anything_else() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    let pane = open_pane(&f);
    live(&f, &pane);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.agent_stopped && v.box_deleted && v.workspace_closed);
    assert!(!live_session(&f));
    let surface_calls = f.surface.calls();
    assert!(index(&surface_calls, "close_pane") < index(&surface_calls, "close_workspace"));
    let driver_calls = f.driver.calls();
    assert!(driver_calls.iter().any(|c| c.starts_with("release")));
}

#[test]
fn a_pane_already_gone_still_ends_the_session() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    live(&f, "p-gone");
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.agent_stopped);
    assert!(!live_session(&f));
    assert_eq!(v.task.state, TaskState::Closed);
}

#[test]
fn a_release_error_keeps_the_task_open_with_the_session_ended() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: "x".into(),
        }),
        FakeSurface::new("/s"),
        0,
    );
    let pane = open_pane(&f);
    live(&f, &pane);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(!live_session(&f));
    assert_eq!(row(&f).state, TaskState::Working);
    assert_eq!(box_state(&f), BoxState::Ready);
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("close_workspace")));
}

#[test]
fn a_task_with_no_session_closes_as_before() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(!v.agent_stopped);
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("close_pane")));
}

#[test]
fn a_pane_close_failure_keeps_the_session_and_the_task() {
    let f = fx_with(
        FakeDriver::new(),
        FakeSurface::new("/s").fail_close_pane(SurfaceError::Timeout),
        0,
    );
    live(&f, "p-1");
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(live_session(&f));
    assert_eq!(row(&f).state, TaskState::Working);
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("release")));
}
