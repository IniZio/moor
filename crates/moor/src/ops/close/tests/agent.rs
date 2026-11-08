use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::DriverError;
use moor_core::seam::surface::{PaneCmd, Surface, SurfaceError};
use moor_core::ErrorKind;

use super::teardown::{exec_calls, YAML};
use super::{box_state, fx_box, fx_repo, fx_with, is_working, req, row, run, Fx};
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
        env: Vec::new(),
    };
    let ws = f.surface.ensure_workspace("t").unwrap();
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
    let f = fx_with(FakeDriver::new(), FakeSurface::new());
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
    let f = fx_with(FakeDriver::new(), FakeSurface::new());
    live(&f, "p-gone");
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.agent_stopped);
    assert!(!live_session(&f));
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
}

#[test]
fn a_release_error_keeps_the_box_open_with_the_session_ended() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: "x".into(),
        }),
        FakeSurface::new(),
    );
    let pane = open_pane(&f);
    live(&f, &pane);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(!live_session(&f));
    assert!(is_working(&row(&f)));
    assert_eq!(box_state(&f), BoxState::Ready);
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("close_workspace")));
}

#[test]
fn a_box_with_no_session_closes_as_before() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new());
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(!v.agent_stopped);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("close_pane")));
}

#[test]
fn a_pane_close_failure_keeps_the_session_and_the_box() {
    let f = fx_with(
        FakeDriver::new(),
        FakeSurface::new().fail_close_pane(SurfaceError::Timeout),
    );
    live(&f, "p-1");
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(live_session(&f));
    assert!(is_working(&row(&f)));
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("release")));
}

#[test]
fn a_refused_gate_leaves_the_agent_running() {
    let f = fx_box(
        FakeDriver::new(),
        FakeSurface::new(),
        Some("ahead=2 dirty=0"),
    );
    let pane = open_pane(&f);
    live(&f, &pane);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(live_session(&f));
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("close_pane")));
}

#[test]
fn a_failed_pane_close_runs_no_teardown() {
    let f = fx_repo(
        FakeDriver::new(),
        FakeSurface::new().fail_close_pane(SurfaceError::Timeout),
        Some("ahead=0 dirty=0"),
        Some(YAML),
    );
    live(&f, "p-1");
    run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(exec_calls(&f), 0);
}

#[test]
fn a_session_without_a_pane_is_ended_without_closing_one() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new());
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", None)
        .unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.agent_stopped);
    assert!(!live_session(&f));
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("close_pane")));
}

#[test]
fn ending_a_session_records_a_sessions_stopped_event() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new());
    live(&f, "p-gone");
    run(&f.cx, &req(false)).unwrap();
    let events = f.cx.store().unwrap().events(f.id, 50).unwrap();
    let stopped: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "sessions_stopped")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(stopped, [serde_json::json!({"sessions": ["s-1"]})]);
}
