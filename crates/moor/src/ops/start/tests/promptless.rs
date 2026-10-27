use std::sync::Arc;

use moor_core::seam::engine::{LaunchPrompt, LaunchReq};
use moor_core::{BoxId, StartReq};

use super::relaunch::{gone, obs, session, set_agent_session, started};
use super::{req, world, World};
use crate::ops::start::{revive_after_wake, run};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// A world whose engine opens pane `p-1` on its first launch.
fn fresh() -> (World, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(session("p-1"))));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

/// The only launch request; fails unless there is exactly one.
fn only(engine: &FakeEngine) -> LaunchReq {
    let mut all = engine.launches();
    assert_eq!(all.len(), 1);
    all.remove(0)
}

/// The second launch request; fails unless there are exactly two.
fn second(engine: &FakeEngine) -> LaunchReq {
    let mut all = engine.launches();
    assert_eq!(all.len(), 2);
    all.remove(1)
}

#[test]
fn a_start_without_a_prompt_launches_the_agent_with_no_first_prompt() {
    let (w, engine) = fresh();
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    let first = only(&engine);
    assert_eq!(first.prompt, LaunchPrompt::None);
}

#[test]
fn a_start_with_an_issue_asks_the_agent_to_resolve_it() {
    let (w, engine) = fresh();
    let issue = StartReq {
        issue: Some("42".into()),
        ..req("demo")
    };
    run(&w.cx, &issue).unwrap();
    assert_eq!(
        only(&engine).prompt,
        LaunchPrompt::Text("Resolve issue 42.".into())
    );
}

#[test]
fn a_start_with_no_agent_and_no_prompt_launches_nothing() {
    let (w, engine) = fresh();
    let none = StartReq {
        no_agent: true,
        ..req("demo")
    };
    assert!(!run(&w.cx, &none).unwrap().agent.launched);
    assert!(engine.launches().is_empty());
}

#[test]
fn a_promptless_start_of_a_live_agent_changes_nothing() {
    let (w, engine, id) = started();
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    assert!(!run(&w.cx, &req("demo")).unwrap().agent.launched);
    assert_eq!(engine.launches().len(), 1);
    let live = w.cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert_eq!(live.pane.as_deref(), Some("p-1"));
}

#[test]
fn a_promptless_start_after_the_pane_ended_resumes_the_stored_session() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    assert!(run(&w.cx, &req("demo")).unwrap().agent.launched);
    let next = second(&engine);
    assert_eq!(next.resume.as_deref(), Some("agent-1"));
    assert_eq!(next.prompt, LaunchPrompt::None);
}

#[test]
fn a_promptless_start_after_the_pane_ended_with_no_stored_session_launches_with_no_prompt() {
    let (w, engine, _id) = started();
    gone(&w);
    assert!(run(&w.cx, &req("demo")).unwrap().agent.launched);
    let next = second(&engine);
    assert_eq!(next.resume, None);
    assert_eq!(next.prompt, LaunchPrompt::None);
}

#[test]
fn a_promptless_start_after_the_session_ended_resumes_the_stored_session() {
    let (w, engine, id) = started();
    let store = w.cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.end_session(live.id, None, None).unwrap();
    set_agent_session(&w, id, "agent-1");
    assert!(run(&w.cx, &req("demo")).unwrap().agent.launched);
    let next = second(&engine);
    assert_eq!(next.resume.as_deref(), Some("agent-1"));
    assert_eq!(next.prompt, LaunchPrompt::None);
}

#[test]
fn a_wake_of_a_box_with_neither_prompt_nor_session_launches_nothing() {
    let (w, engine) = fresh();
    let none = StartReq {
        no_agent: true,
        ..req("demo")
    };
    let id = BoxId::new(run(&w.cx, &none).unwrap().boxed.id);
    revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
    assert!(engine.launches().is_empty());
}
