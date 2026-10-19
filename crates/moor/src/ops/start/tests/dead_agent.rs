use std::sync::Arc;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::domain::task::TaskState;
use moor_core::{BoxId, BoxName, ErrorKind, ShowReq, StartReq, WaitFor, WaitReq};

use super::relaunch::{gone, session, started};
use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

const NOT_FOUND: &str = "the agent exited 127: its command was not found on the Box";

fn goal_req(name: &str) -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req(name)
    }
}

/// A pane observation of an agent that exited with `code` after printing `tail`.
fn exited(code: i32, tail: &str) -> PaneObs {
    PaneObs {
        status: PaneStatus::Unknown,
        tail: tail.to_owned(),
        prompt: None,
        exit_marker: Some(code),
    }
}

fn launching() -> World {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(session("p-1"))));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine);
    w
}

#[test]
fn an_agent_that_exits_127_at_launch_is_a_launch_failure() {
    let w = launching();
    w.surface.push_pane_obs(
        "p-1",
        Ok(exited(127, "sh: claude: not found\nmoor: agent exited 127")),
    );
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    let id = BoxId::new(v.task.id);
    let store = w.cx.store().unwrap();
    assert!(!v.agent.launched);
    assert!(v.agent.session.is_none());
    let f = v.agent.reason.unwrap();
    assert_eq!((f.step.as_str(), f.message.as_str()), ("launch", NOT_FOUND));
    assert_eq!(
        f.tail.as_deref(),
        Some("sh: claude: not found\nmoor: agent exited 127")
    );
    assert_eq!(
        v.warnings_inline,
        [format!(
            "agent dispatch failed: launch: {NOT_FOUND}; re-run start to retry"
        )]
    );
    assert!(store.task(id).unwrap().unwrap().launch_failure.is_some());
    assert!(store.live_session(id).unwrap().is_none());
    let events = store.events(id, 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "session_failed").unwrap();
    assert_eq!(
        failed.payload,
        serde_json::json!({"step": "launch", "message": NOT_FOUND})
    );
}

#[test]
fn any_other_exit_code_is_named_in_the_failure() {
    let w = launching();
    w.surface.push_pane_obs("p-1", Ok(exited(2, "")));
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    let f = v.agent.reason.unwrap();
    assert_eq!(f.message, "the agent exited 2 right after launch");
    assert!(f.tail.is_none());
    assert!(!v.agent.launched);
}

#[test]
fn a_dead_agent_shows_as_failed_and_wait_fails() {
    let w = launching();
    w.surface.push_pane_obs("p-1", Ok(exited(127, "")));
    run(&w.cx, &goal_req("demo")).unwrap();
    let show = ShowReq {
        task: None,
        events: None,
        agent: true,
        reports: false,
        artifacts: false,
        pane: false,
    };
    let shown = crate::ops::show::run(&w.cx, &show).unwrap();
    let t = shown.tasks.as_ref().unwrap().first().unwrap();
    assert_eq!(t.state, TaskState::Working);
    assert_eq!(t.agent.as_ref().unwrap().state, "failed");
    let wait = WaitReq {
        task: BoxName::parse_allow_bare("demo").unwrap(),
        wait_for: Some(WaitFor::Idle),
        timeout_secs: Some(60),
    };
    let e = crate::ops::wait::run(&w.cx, &wait).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(
        e.error.to_string(),
        format!("agent dispatch failed: launch: {NOT_FOUND}; re-run start to retry")
    );
}

#[test]
fn a_relaunched_agent_that_exits_127_is_a_launch_failure() {
    let (w, _engine, id) = started();
    gone(&w);
    w.surface.push_pane_obs("p-2", Ok(exited(127, "")));
    let v = run(&w.cx, &req("demo")).unwrap();
    let store = w.cx.store().unwrap();
    assert!(!v.agent.launched);
    assert_eq!(v.agent.reason.unwrap().message, NOT_FOUND);
    assert!(store.task(id).unwrap().unwrap().launch_failure.is_some());
    assert!(store.live_session(id).unwrap().is_none());
    let events = store.events(id, 50).unwrap();
    assert!(events.iter().any(|e| e.kind == "session_failed"));
}
