use std::path::PathBuf;
use std::sync::Arc;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::agent_profile::FirstRunPrompt;
use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};
use moor_core::seam::surface::{PaneRef, ResumeOutcome, SurfaceError};
use moor_core::{BoxHandle, BoxId, BoxPath, ErrorKind, StartReq};

use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// A start request for `demo` with `goal`.
fn goal_req(goal: &str) -> StartReq {
    StartReq {
        goal: Some(goal.into()),
        ..req("demo")
    }
}

/// A launched session whose pane is `pane`.
pub(super) fn session(pane: &str) -> SessionRef {
    SessionRef {
        id: String::new(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/wt"),
        pane: Some(PaneRef::new(pane)),
        warnings: Vec::new(),
    }
}

/// A pane observation of a running agent, or of one that exited with `code`.
pub(super) fn obs(exit_marker: Option<i32>) -> PaneObs {
    PaneObs {
        status: PaneStatus::Working,
        tail: String::new(),
        prompt: None,
        exit_marker,
    }
}

/// A world with a Working task `demo` launched with "ship it" in pane `p-1`; the next launch opens `p-2`.
pub(super) fn started() -> (World, Arc<FakeEngine>, BoxId) {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2"))),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &goal_req("ship it")).unwrap();
    (w, engine, BoxId::new(first.task.id))
}

/// The second launch request; fails unless there are exactly two.
fn relaunch_of(engine: &FakeEngine) -> LaunchReq {
    let mut all = engine.launches();
    assert_eq!(all.len(), 2);
    all.remove(1)
}

pub(super) fn set_agent_session(w: &World, id: BoxId, agent: &str) {
    let set = Fields {
        agent_session_id: Field::Set(agent.into()),
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(set))
        .unwrap();
}

fn live_pane(w: &World, id: BoxId) -> Option<String> {
    w.cx.store()
        .unwrap()
        .live_session(id)
        .unwrap()
        .unwrap()
        .pane
}

fn exit_of_first_session(w: &World, id: BoxId) -> Option<i32> {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT exit FROM launches WHERE box = ? ORDER BY id LIMIT 1",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

/// Makes `p-1` read as gone.
pub(super) fn gone(w: &World) {
    w.surface.push_pane_obs("p-1", Err(SurfaceError::NotFound));
}

fn warning_codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

#[test]
fn a_gone_pane_relaunches_with_the_newest_goal() {
    let (w, engine, id) = started();
    gone(&w);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    let second = relaunch_of(&engine);
    assert_eq!(second.goal.as_deref(), Some("ship it"));
    assert_eq!(second.resume, None);
    assert_eq!(live_pane(&w, id).as_deref(), Some("p-2"));
}

#[test]
fn an_exited_pane_relaunches_and_the_old_session_keeps_its_exit() {
    let (w, engine, id) = started();
    w.surface.push_pane_obs("p-1", Ok(obs(Some(3))));
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(live_pane(&w, id).as_deref(), Some("p-2"));
    assert_eq!(exit_of_first_session(&w, id), Some(3));
    assert!(w
        .surface
        .calls()
        .iter()
        .all(|c| !c.starts_with("close_pane")));
}

#[test]
fn a_given_goal_relaunches_with_a_fresh_brief_and_is_stored() {
    let (w, engine, id) = started();
    gone(&w);
    set_agent_session(&w, id, "agent-1");
    run(&w.cx, &goal_req("new goal")).unwrap();
    let second = relaunch_of(&engine);
    assert_eq!(second.goal.as_deref(), Some("new goal"));
    assert_eq!(second.resume, None);
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(row.goal.as_deref(), Some("new goal"));
}

#[test]
fn a_stored_agent_session_relaunches_with_resume() {
    let (w, engine, id) = started();
    gone(&w);
    set_agent_session(&w, id, "agent-1");
    run(&w.cx, &req("demo")).unwrap();
    let second = relaunch_of(&engine);
    assert_eq!(second.resume.as_deref(), Some("agent-1"));
    assert_eq!(live_pane(&w, id).as_deref(), Some("p-2"));
}

#[test]
fn an_empty_stored_agent_session_uses_the_brief() {
    let (w, engine, id) = started();
    gone(&w);
    set_agent_session(&w, id, "");
    run(&w.cx, &req("demo")).unwrap();
    let second = relaunch_of(&engine);
    assert_eq!(second.resume, None);
    assert_eq!(second.goal.as_deref(), Some("ship it"));
}

#[test]
fn a_pane_read_error_counts_as_live() {
    let (w, engine, id) = started();
    w.surface.push_pane_obs("p-1", Err(SurfaceError::Timeout));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(live_pane(&w, id).as_deref(), Some("p-1"));
}

#[test]
fn a_given_goal_with_a_live_pane_warns_and_changes_nothing() {
    let (w, engine, id) = started();
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    w.cx.take_warnings();
    run(&w.cx, &goal_req("new goal")).unwrap();
    let warned = w.cx.take_warnings();
    let msg = warned.iter().find(|x| x.code == "goal_ignored").unwrap();
    assert_eq!(
        msg.message,
        "--goal ignored: the agent for demo is live; use moor steer"
    );
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(live_pane(&w, id).as_deref(), Some("p-1"));
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(row.goal.as_deref(), Some("ship it"));
}

#[test]
fn a_live_pane_without_a_goal_does_not_warn() {
    let (w, _engine, _id) = started();
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    w.cx.take_warnings();
    run(&w.cx, &req("demo")).unwrap();
    assert!(!warning_codes(&w).contains(&"goal_ignored".to_owned()));
}

#[test]
fn the_relaunched_event_carries_both_panes_and_the_goal() {
    let (w, _engine, id) = started();
    gone(&w);
    run(&w.cx, &goal_req("new goal")).unwrap();
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    let e = events.iter().find(|e| e.kind == "relaunched").unwrap();
    assert_eq!(
        e.payload,
        serde_json::json!({"old_pane": "p-1", "new_pane": "p-2", "goal": "new goal"})
    );
}

#[test]
fn a_failed_relaunch_records_no_relaunched_event_and_sets_the_failure() {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Err(EngineError::Failed {
                step: "pane",
                message: "no tab".to_owned(),
            })),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine);
    let id = BoxId::new(run(&w.cx, &goal_req("ship it")).unwrap().task.id);
    gone(&w);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    let store = w.cx.store().unwrap();
    assert!(store.task(id).unwrap().unwrap().launch_failure.is_some());
    let events = store.events(id, 100).unwrap();
    assert!(events.iter().all(|e| e.kind != "relaunched"));
}

#[test]
fn a_gone_pane_with_no_goal_and_no_agent_session_launches_nothing() {
    let (w, engine, id) = started();
    let clear = Fields {
        goal: Field::Clear,
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(clear))
        .unwrap();
    gone(&w);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_gone_pane_start_is_refused_while_another_op_holds_the_lock() {
    let (w, engine, _id) = started();
    gone(&w);
    let held = super::hold(&w, "close");
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert_eq!(engine.launches().len(), 1);
    drop(held);
    assert!(run(&w.cx, &req("demo")).unwrap().agent.launched);
}

#[test]
fn a_relaunch_watches_its_new_pane_for_a_first_run_prompt() {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2")))
            .prompt("trust this folder", FirstRunPrompt::FolderTrust),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine);
    let first = run(&w.cx, &goal_req("ship it")).unwrap();
    let id = BoxId::new(first.task.id);
    gone(&w);
    w.surface.push_pane_obs(
        "p-2",
        Ok(PaneObs {
            tail: "Do you trust this folder?".to_owned(),
            ..obs(None)
        }),
    );
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let blocked = events
        .iter()
        .find(|e| e.kind == "agent_prompt_blocked")
        .unwrap();
    assert_eq!(blocked.payload.get("pane"), Some(&"p-2".into()));
    assert_eq!(blocked.payload.get("prompt"), Some(&"folder_trust".into()));
}

/// A world whose Working task `demo` has no goal and no session.
fn goalless() -> (World, Arc<FakeEngine>, BoxId) {
    let (w, engine, id) = started();
    let clear = Fields {
        goal: Field::Clear,
        ..Fields::default()
    };
    let store = w.cx.store().unwrap();
    store.apply(Decision::new(id).set(clear)).unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.end_session(live.id, None, None).unwrap();
    (w, engine, id)
}

#[test]
fn a_goal_on_a_goalless_task_without_a_session_is_stored_and_launches_once() {
    let (w, engine, id) = goalless();
    let v = run(&w.cx, &goal_req("late goal")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(relaunch_of(&engine).goal.as_deref(), Some("late goal"));
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(row.goal.as_deref(), Some("late goal"));
}

#[test]
fn a_second_start_without_a_goal_does_not_launch_again() {
    let (w, engine, _id) = goalless();
    run(&w.cx, &goal_req("late goal")).unwrap();
    w.surface.push_pane_obs("p-2", Ok(obs(None)));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
}

#[test]
fn an_issue_alone_does_not_launch_a_goalless_task() {
    let (w, engine, _id) = goalless();
    let issue = StartReq {
        issue: Some("42".into()),
        ..req("demo")
    };
    let v = run(&w.cx, &issue).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_goal_on_a_goalless_task_with_a_live_pane_warns() {
    let (w, engine, id) = started();
    let clear = Fields {
        goal: Field::Clear,
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(clear))
        .unwrap();
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    w.cx.take_warnings();
    run(&w.cx, &goal_req("late goal")).unwrap();
    assert!(warning_codes(&w).contains(&"goal_ignored".to_owned()));
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_given_goal_replaces_the_stored_goal_when_there_is_no_session() {
    let (w, engine, id) = started();
    let store = w.cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.end_session(live.id, None, None).unwrap();
    let v = run(&w.cx, &goal_req("second goal")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(relaunch_of(&engine).goal.as_deref(), Some("second goal"));
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(row.goal.as_deref(), Some("second goal"));
}

/// A resume request for `demo`.
fn resume_req() -> StartReq {
    StartReq {
        resume: true,
        ..req("demo")
    }
}

/// The resume registrations the surface saw.
fn registrations(w: &World) -> Vec<String> {
    w.surface
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("register_resume"))
        .collect()
}

#[test]
fn resume_without_a_stored_session_is_refused() {
    let (w, engine, _id) = started();
    gone(&w);
    let e = run(&w.cx, &resume_req()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(e.error.to_string(), "no agent session to resume");
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn resume_with_an_empty_stored_session_is_refused() {
    let (w, _engine, id) = started();
    set_agent_session(&w, id, "");
    let e = run(&w.cx, &resume_req()).unwrap_err();
    assert_eq!(e.error.to_string(), "no agent session to resume");
}

#[test]
fn resume_of_a_task_that_does_not_exist_is_refused() {
    let w = world(FakeDriver::new());
    let e = run(&w.cx, &resume_req()).unwrap_err();
    assert_eq!(e.error.to_string(), "no agent session to resume");
}

#[test]
fn resume_on_a_live_pane_registers_again_and_does_not_launch() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.have_pane("p-1");
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    let live = w.cx.store().unwrap().live_session(id).unwrap().unwrap();
    w.cx.store()
        .unwrap()
        .set_registered_session(live.id, "agent-1")
        .unwrap();
    let v = run(&w.cx, &resume_req()).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(registrations(&w), ["register_resume p-1 claude agent-1"]);
}

#[test]
fn a_live_resume_records_the_registration_and_a_later_settle_sends_no_second_one() {
    let (w, _engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.have_pane("p-1");
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    run(&w.cx, &resume_req()).unwrap();
    let live = w.cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert_eq!(live.registered_agent_session.as_deref(), Some("agent-1"));
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(registrations(&w), ["register_resume p-1 claude agent-1"]);
}

#[test]
fn resume_on_a_gone_pane_launches_the_resume_and_registers_the_new_pane() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.have_pane("p-2");
    gone(&w);
    let v = run(&w.cx, &resume_req()).unwrap();
    assert!(v.agent.launched);
    assert_eq!(relaunch_of(&engine).resume.as_deref(), Some("agent-1"));
    assert_eq!(registrations(&w), ["register_resume p-2 claude agent-1"]);
}

#[test]
fn a_fresh_launch_registers_nothing_without_a_session_id() {
    let (w, _engine, _id) = started();
    assert!(registrations(&w).is_empty());
}

#[test]
fn a_relaunch_that_resumes_registers_the_new_pane() {
    let (w, _engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.have_pane("p-2");
    gone(&w);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(registrations(&w), ["register_resume p-2 claude agent-1"]);
}

#[test]
fn a_rejected_registration_is_a_warning_not_a_failure() {
    let (w, _engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.have_pane("p-1");
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    w.surface
        .set_resume_reply(ResumeOutcome::Rejected("invalid_resume_argv".into()));
    w.cx.take_warnings();
    run(&w.cx, &resume_req()).unwrap();
    let warned = w.cx.take_warnings();
    let msg = warned
        .iter()
        .find(|x| x.code == "resume_not_registered")
        .unwrap();
    assert!(msg.message.ends_with("invalid_resume_argv"), "{msg:?}");
}

/// A world with a Working goal-less task `demo`, no session, and stored agent session `agent-1`.
fn idle_of_sessions() -> (World, Arc<FakeEngine>, BoxId) {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(session("p-1"))));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(
        &w.cx,
        &StartReq {
            no_agent: true,
            ..req("demo")
        },
    )
    .unwrap();
    let id = BoxId::new(first.task.id);
    set_agent_session(&w, id, "agent-1");
    (w, engine, id)
}

#[test]
fn resume_with_no_session_row_launches_the_stored_session() {
    let (w, engine, _id) = idle_of_sessions();
    let v = run(&w.cx, &resume_req()).unwrap();
    assert!(v.agent.launched);
    let all = engine.launches();
    assert_eq!(all.len(), 1);
    assert_eq!(
        all.first().and_then(|l| l.resume.as_deref()),
        Some("agent-1")
    );
}

#[test]
fn a_goal_beats_resume_when_there_is_no_session_row() {
    let (w, engine, _id) = idle_of_sessions();
    let r = StartReq {
        goal: Some("G".into()),
        ..resume_req()
    };
    run(&w.cx, &r).unwrap();
    let all = engine.launches();
    let first = all.first().unwrap();
    assert_eq!(first.resume, None);
    assert_eq!(first.goal.as_deref(), Some("G"));
}

#[test]
fn resume_with_a_goal_is_not_refused_without_a_stored_session() {
    let (w, engine, id) = idle_of_sessions();
    set_agent_session(&w, id, "");
    let r = StartReq {
        goal: Some("G".into()),
        ..resume_req()
    };
    let v = run(&w.cx, &r).unwrap();
    assert!(v.agent.launched);
    assert_eq!(
        engine.launches().first().and_then(|l| l.goal.as_deref()),
        Some("G")
    );
}

#[test]
fn a_surface_error_while_registering_is_a_warning() {
    let (w, _engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    w.cx.take_warnings();
    run(&w.cx, &resume_req()).unwrap();
    assert!(warning_codes(&w).contains(&"resume_not_registered".to_owned()));
}
