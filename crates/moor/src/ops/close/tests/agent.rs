use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::DriverError;
use moor_core::ErrorKind;

use super::{box_state, fx_box, fx_with, is_working, req, row, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

fn live(f: &Fx) {
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", None)
        .unwrap();
}

fn with_engine(f: Fx) -> (Fx, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::new());
    let f = Fx {
        cx: f.cx.with_engine(engine.clone()),
        ..f
    };
    (f, engine)
}

fn live_session(f: &Fx) -> bool {
    f.cx.store().unwrap().live_session(f.id).unwrap().is_some()
}

#[test]
fn close_stops_the_agent_before_anything_else() {
    let (f, engine) = with_engine(fx_with(FakeDriver::new()));
    live(&f);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.agent_stopped && v.box_deleted);
    assert!(!live_session(&f));
    assert!(engine.asked().contains(&"stop"));
    assert!(f.driver.calls().iter().any(|c| c.starts_with("release")));
}

#[test]
fn a_release_error_keeps_the_box_open_with_the_session_ended() {
    let f = fx_with(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: "x".into(),
    }));
    live(&f);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(!live_session(&f));
    assert!(is_working(&row(&f)));
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn a_box_with_no_session_closes_as_before() {
    let (f, engine) = with_engine(fx_with(FakeDriver::new()));
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(!v.agent_stopped);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(!engine.asked().contains(&"stop"));
}

#[test]
fn a_refused_gate_leaves_the_agent_running() {
    let f = fx_box(FakeDriver::new(), Some("ahead=2 dirty=0"));
    let (f, engine) = with_engine(f);
    live(&f);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(live_session(&f));
    assert!(!engine.asked().contains(&"stop"));
}

#[test]
fn ending_a_session_records_a_sessions_stopped_event() {
    let f = fx_with(FakeDriver::new());
    live(&f);
    run(&f.cx, &req(false)).unwrap();
    let events = f.cx.store().unwrap().events(f.id, 50).unwrap();
    let stopped: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "sessions_stopped")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(stopped, [serde_json::json!({"sessions": ["s-1"]})]);
}
