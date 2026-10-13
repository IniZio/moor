use std::sync::Arc;

use moor_core::domain::decision::Decision;
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::engine::EngineError;
use moor_core::{ErrorKind, ShowReq, StartReq, TaskId, TaskName, WaitFor, WaitReq};

use super::{req, world, world_at, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// A start request for `name` with a goal.
fn goal_req(name: &str) -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req(name)
    }
}

#[test]
fn start_records_one_live_session_and_attach_adds_none() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let store = w.cx.store().unwrap();
    let sessions = || -> i64 {
        store
            .reader()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE task = ? AND ended_at IS NULL",
                [id.get()],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(first.task.state, TaskState::Working);
    assert_eq!(sessions(), 1);
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(sessions(), 1);
}

#[test]
fn start_without_a_goal_records_no_session() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(first.task.state, TaskState::Working);
    assert!(!w.cx.store().unwrap().has_session(id).unwrap());
}

#[test]
fn wait_for_idle_returns_after_a_start_without_a_goal() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    let wait = WaitReq {
        task: TaskName::parse("demo").unwrap(),
        wait_for: Some(WaitFor::Idle),
        timeout_secs: Some(60),
    };
    let v = crate::ops::wait::run(&w.cx, &wait).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.waited_ms, 0);
}

/// A world whose engine is `engine`.
fn with_engine(engine: &Arc<FakeEngine>) -> World {
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    w
}

/// An engine failure at `step`.
fn launch_error(step: &'static str, message: &str) -> EngineError {
    EngineError::Failed {
        step,
        message: message.to_owned(),
    }
}

#[test]
fn a_launch_records_a_session_with_its_pane() {
    let engine = Arc::new(FakeEngine::new());
    let w = with_engine(&engine);
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(v.task.id);
    assert!(v.agent.launched);
    let s = w.cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert_eq!(s.pane.as_deref(), Some("p-1"));
    assert_eq!(v.agent.session.as_deref(), Some(s.session_ref.as_str()));
    let launches = engine.launches();
    let l = launches.first().unwrap();
    assert_eq!(l.goal.as_deref(), Some("ship it"));
    assert_eq!(l.task.as_str(), "demo");
    assert_eq!(l.setup_exit, None);
}

#[test]
fn a_failed_launch_records_no_session_and_leaves_the_task_working() {
    let engine = Arc::new(
        FakeEngine::new().launch_result(Err(launch_error("pane", "no tab\nline one\nline two"))),
    );
    let w = with_engine(&engine);
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(v.task.id);
    let store = w.cx.store().unwrap();
    assert_eq!(v.task.state, TaskState::Working);
    assert!(!v.agent.launched);
    assert!(!store.has_session(id).unwrap());
    let f = v.agent.reason.unwrap();
    assert_eq!((f.step.as_str(), f.message.as_str()), ("pane", "no tab"));
    assert_eq!(f.tail.as_deref(), Some("line one\nline two"));
    assert_eq!(
        v.warnings_inline,
        ["agent dispatch failed: pane: no tab; re-run start to retry"]
    );
    assert!(store.task(id).unwrap().unwrap().launch_failure.is_some());
}

#[test]
fn a_launch_failure_shows_as_failed_and_is_not_idled_by_the_next_list() {
    let engine = Arc::new(FakeEngine::new().launch_result(Err(launch_error("pane", "no tab"))));
    let w = with_engine(&engine);
    run(&w.cx, &goal_req("demo")).unwrap();
    let req = ShowReq {
        task: None,
        events: None,
        agent: true,
        reports: false,
        pane: false,
    };
    let shown = crate::ops::show::run(&w.cx, &req).unwrap();
    let t = shown.tasks.first().unwrap();
    assert_eq!(t.state, TaskState::Working);
    let a = t.agent.as_ref().unwrap();
    assert_eq!(a.state, "failed");
    assert!(!a.launched);
    assert_eq!(a.reason.as_ref().unwrap().message, "no tab");
}

#[test]
fn wait_fails_on_a_launch_failure() {
    let engine = Arc::new(FakeEngine::new().launch_result(Err(launch_error("pane", "no tab"))));
    let w = with_engine(&engine);
    run(&w.cx, &goal_req("demo")).unwrap();
    let wait = WaitReq {
        task: TaskName::parse("demo").unwrap(),
        wait_for: Some(WaitFor::Idle),
        timeout_secs: Some(60),
    };
    let e = crate::ops::wait::run(&w.cx, &wait).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(
        e.error.to_string(),
        "agent dispatch failed: pane: no tab; re-run start to retry"
    );
}

#[test]
fn starting_again_launches_once_and_clears_the_failure() {
    let engine = Arc::new(FakeEngine::new().launch_result(Err(launch_error("pane", "no tab"))));
    let w = with_engine(&engine);
    let first = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let again = run(&w.cx, &goal_req("demo")).unwrap();
    assert!(again.agent.launched);
    assert!(again.agent.reason.is_none());
    let store = w.cx.store().unwrap();
    assert!(store.task(id).unwrap().unwrap().launch_failure.is_none());
    assert!(store.live_session(id).unwrap().is_some());
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(engine.launches().len(), 2);
}

#[test]
fn restarting_an_idle_task_with_a_goal_and_no_session_wakes_and_launches_once() {
    let engine = Arc::new(FakeEngine::new());
    let w = with_engine(&engine);
    let first = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let store = w.cx.store().unwrap();
    while let Some(live) = store.live_session(id).unwrap() {
        store.end_session(live.id, Some(0), None).unwrap();
    }
    store
        .apply(Decision::new(id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    assert_eq!(store.task(id).unwrap().unwrap().state, TaskState::Idle);
    let again = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(again.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    let events = store.events(id, 100).unwrap();
    assert!(events.iter().any(|e| e.kind == "busy"));
}

#[test]
fn restarting_an_idle_task_without_a_goal_or_session_wakes_it_and_launches_nothing() {
    let engine = Arc::new(FakeEngine::new());
    let w = with_engine(&engine);
    let first = run(&w.cx, &req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    let again = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(store
        .events(id, 100)
        .unwrap()
        .iter()
        .any(|e| e.kind == "busy"));
    assert!(engine.launches().is_empty());
}

#[test]
fn rerunning_after_a_failed_launch_launches_once_and_stays_working() {
    let engine = Arc::new(FakeEngine::new().launch_result(Err(launch_error("pane", "no tab"))));
    let w = with_engine(&engine);
    let first = run(&w.cx, &goal_req("demo")).unwrap();
    let id = TaskId::new(first.task.id);
    let again = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(again.task.state, TaskState::Working);
    assert!(again.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    let store = w.cx.store().unwrap();
    let busy = store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "busy")
        .count();
    assert_eq!(busy, 0);
}

#[test]
fn no_agent_and_no_goal_launch_nothing() {
    let engine = Arc::new(FakeEngine::new());
    let w = with_engine(&engine);
    let flagged = StartReq {
        no_agent: true,
        ..goal_req("a")
    };
    run(&w.cx, &flagged).unwrap();
    run(&w.cx, &req("b")).unwrap();
    assert!(engine.launches().is_empty());
}

#[test]
fn a_failed_setup_still_launches_with_its_exit_code() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [bad]\n").unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 3 })]);
    let engine = Arc::new(FakeEngine::new());
    let mut w = world_at(driver, repo.path());
    w.cx = w.cx.with_engine(engine.clone());
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    assert!(!v.setup.ok);
    assert!(v.agent.launched);
    assert_eq!(engine.launches().first().unwrap().setup_exit, Some(3));
}

#[test]
fn a_launch_failure_stays_in_the_events_after_a_successful_retry() {
    let engine = Arc::new(FakeEngine::new().launch_result(Err(launch_error("pane", "no tab"))));
    let w = with_engine(&engine);
    let id = TaskId::new(run(&w.cx, &goal_req("demo")).unwrap().task.id);
    run(&w.cx, &goal_req("demo")).unwrap();
    let kinds: Vec<_> =
        w.cx.store()
            .unwrap()
            .events(id, 50)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.payload))
            .collect();
    let failed = kinds.iter().find(|(k, _)| k == "session_failed").unwrap();
    assert_eq!(
        failed.1,
        serde_json::json!({"step": "pane", "message": "no tab"})
    );
    assert!(kinds.iter().any(|(k, _)| k == "session_started"));
}

#[test]
fn waiting_for_a_run_does_not_return_the_dispatch_failure() {
    let engine = Arc::new(FakeEngine::new().launch_result(Err(launch_error("pane", "no tab"))));
    let w = with_engine(&engine);
    run(&w.cx, &goal_req("demo")).unwrap();
    let wait = WaitReq {
        task: TaskName::parse("demo").unwrap(),
        wait_for: Some(WaitFor::Run),
        timeout_secs: Some(60),
    };
    let v = crate::ops::wait::run(&w.cx, &wait).unwrap();
    assert!(!v.timed_out);
}
