//! A steer to an awake Box whose agent exited relaunches the agent once, then sends.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::time::Duration;

use moor_core::activity::{HookRead, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::session_state::SessionState;
use moor_core::seam::engine::{EngineError, SessionRef, Steer};
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::{BoxHandle, BoxPath, ErrorKind};

use super::{blocked, fx_with, menu, steered, text, Fx};
use crate::ops::start::revive_for_steer;
use crate::ops::steer::run;
use crate::ops::steer::{relaunch, Gone};
use crate::testing::engine::FakeEngine;

/// Probes that read an agent whose pane shows it exited 3.
fn exited() -> Probes {
    Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Idle,
            tail: String::new(),
            prompt: None,
            exit_marker: Some(3),
        }),
    }
}

/// The session the relaunch starts, `agent-2` in pane `p-2`.
fn relaunched_session() -> SessionRef {
    SessionRef {
        id: "agent-2".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        pane: Some(PaneRef::new("p-2")),
        warnings: Vec::new(),
    }
}

/// Box `t` with an agent in pane `p-1` that has an agent session to resume and a workspace.
fn resumable(engine: FakeEngine) -> Fx {
    let f = fx_with(engine.launch_result(Ok(relaunched_session())), true);
    let set = Fields {
        agent_session_id: Field::Set("stored-1".into()),
        workspace_id: Field::Set("ws-1".into()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(set))
        .unwrap();
    f.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
    f
}

fn relaunches(f: &Fx) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

fn session_state(f: &Fx) -> SessionState {
    f.cx.store()
        .unwrap()
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .session
}

#[test]
fn a_gone_agent_is_relaunched_with_its_stored_session_then_steered() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(v.sent, "hi");
    let launches = f.engine.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].resume.as_deref(), Some("stored-1"));
    assert_eq!(launches[0].workspace.as_str(), "ws-1");
    let causes: Vec<_> = relaunches(&f).iter().map(|e| e["cause"].clone()).collect();
    assert_eq!(causes, [serde_json::json!("steer")]);
    let sends = f.engine.steers();
    assert_eq!(
        sends.len(),
        2,
        "one failed send, one to the relaunched agent"
    );
    assert_eq!(sends[1].0, "agent-2");
    assert_eq!(steered(&f).len(), 1);
}

#[test]
fn an_agent_gone_again_after_its_relaunch_fails_the_steer_and_ends_the_session() {
    let engine = FakeEngine::new()
        .steer_result(Err(EngineError::PaneGone))
        .steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let message = err.error.to_string();
    assert!(
        message.contains("again within 60s of its relaunch"),
        "{message}"
    );
    assert_eq!(f.engine.launches().len(), 1, "relaunched once only");
    assert_eq!(session_state(&f), SessionState::Ended);
    assert!(steered(&f).is_empty());
}

#[test]
fn a_second_steer_within_a_minute_of_the_relaunch_does_not_relaunch_again() {
    let engine = FakeEngine::new()
        .steer_result(Err(EngineError::PaneGone))
        .steer_result(Ok(moor_core::seam::engine::SteerOutcome {
            submit_retried: false,
            submit_checked: true,
        }))
        .steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    run(&f.cx, &text("one")).unwrap();
    let err = run(&f.cx, &text("two")).unwrap_err();
    assert!(
        err.error.to_string().contains("again within 60s"),
        "{}",
        err.error
    );
    assert_eq!(f.engine.launches().len(), 1);
    assert_eq!(session_state(&f), SessionState::Ended);
}

#[test]
fn a_gone_agent_a_minute_after_its_relaunch_is_relaunched_again() {
    let engine = FakeEngine::new()
        .steer_result(Err(EngineError::PaneGone))
        .steer_result(Ok(moor_core::seam::engine::SteerOutcome {
            submit_retried: false,
            submit_checked: true,
        }))
        .steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    run(&f.cx, &text("one")).unwrap();
    let reported = Fields {
        agent_session_id: Field::Set("stored-1".into()),
        ..Fields::default()
    };
    let store = f.cx.store().unwrap();
    store.apply(Decision::new(f.id).set(reported)).unwrap();
    f.surface.push_pane_obs("p-2", Err(SurfaceError::NotFound));
    f.cx.clock().sleep(Duration::from_secs(61));
    run(&f.cx, &text("two")).unwrap();
    assert_eq!(f.engine.launches().len(), 2);
}

#[test]
fn an_answer_to_an_exited_agent_relaunches_it_and_picks_the_choice() {
    let engine = FakeEngine::new()
        .probes("agent-1", exited())
        .probes("agent-2", blocked("Allow edit?"));
    let f = resumable(engine);
    run(&f.cx, &menu(1)).unwrap();
    assert_eq!(f.engine.launches().len(), 1);
    let sends = f.engine.steers();
    assert_eq!(sends.len(), 1, "nothing goes to the exited agent");
    assert_eq!(sends[0], ("agent-2".to_owned(), Steer::Menu(1)));
}

#[test]
fn a_steer_that_waited_out_another_relaunch_sends_to_the_new_agent() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    let store = f.cx.store().unwrap();
    let old = store.live_session(f.id).unwrap();
    run(&f.cx, &text("one")).unwrap();
    assert_eq!(session_state(&f), SessionState::Working);
    let gone = Gone::new("t", old, None);
    relaunch::once(&f.cx, store, (f.id, "t"), gone).unwrap();
    assert_eq!(session_state(&f), SessionState::Working);
    assert!(store.live_session(f.id).unwrap().is_some());
    run(&f.cx, &text("two")).unwrap();
    assert_eq!(f.engine.launches().len(), 1);
    assert_eq!(steered(&f).len(), 2);
}

#[test]
fn an_asleep_box_gets_no_new_session_from_a_steer_relaunch() {
    let f = resumable(FakeEngine::new());
    let store = f.cx.store().unwrap();
    assert!(store.end_current_session(f.id, "exited", None).unwrap());
    put_to_sleep_ended(&f);
    let before = store.box_sessions(f.id).unwrap().len();
    let view = revive_for_steer(&f.cx, store, f.id).unwrap();
    assert!(!view.launched);
    assert_eq!(store.box_sessions(f.id).unwrap().len(), before);
}

/// Puts the machine and Box to sleep after the Session ended.
fn put_to_sleep_ended(f: &Fx) {
    let d = Decision::new(f.id)
        .lifecycle(moor_core::domain::box_state::BoxLifecycleInput::Sleep { auto: false })
        .box_input(moor_core::domain::boxes::BoxInput::Sleep);
    f.cx.store().unwrap().apply(d).unwrap();
}

/// Records `n` automatic relaunches of the agent of Box `f`.
fn record_relaunches(f: &Fx, n: usize) {
    for _ in 0..n {
        let event = ExtraEvent::Relaunched {
            old_pane: None,
            new_pane: None,
            goal: None,
            cause: Some("wake".to_owned()),
        };
        f.cx.store()
            .unwrap()
            .apply(Decision::new(f.id).event(event))
            .unwrap();
    }
}

fn kinds(f: &Fx, kind: &str) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_fourth_automatic_relaunch_within_ten_minutes_is_refused_and_recorded_once() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    record_relaunches(&f, 3);
    f.cx.clock().sleep(Duration::from_secs(90));
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    let message = err.error.to_string();
    assert!(
        message.contains("3 times in the last 10 minutes"),
        "{message}"
    );
    assert!(f.engine.launches().is_empty(), "no agent was launched");
    assert_eq!(
        kinds(&f, "relaunch_capped"),
        [serde_json::json!({"count": 3, "window_secs": 600})]
    );
}

#[test]
fn relaunches_older_than_ten_minutes_do_not_count() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let f = resumable(engine);
    record_relaunches(&f, 3);
    f.cx.clock().sleep(Duration::from_mins(11));
    run(&f.cx, &text("hi")).unwrap();
    assert_eq!(f.engine.launches().len(), 1);
    assert!(kinds(&f, "relaunch_capped").is_empty());
}
