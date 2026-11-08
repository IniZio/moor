use std::sync::Arc;

use moor_core::activity::Probe;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::agent_adapter::FirstRunPrompt;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{EngineError, LaunchPrompt, LaunchReq, SessionRef};
use moor_core::{BoxHandle, BoxId, BoxPath, ErrorKind, SessionId, StartReq};

use super::{req, world, World};
use crate::ops::start::run;
use crate::store::db::{Conn, DbExt as _};
use crate::testing::agent;
use crate::testing::db::column_where;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use moor_schema::tables::{Launches, Sessions};
use sea_query::Order;

/// A start request for `demo` with `prompt`.
pub(super) fn prompt_req(prompt: &str) -> StartReq {
    StartReq {
        prompt: Some(prompt.into()),
        ..req("demo")
    }
}

/// A launched session whose reference is `id`.
pub(super) fn session(id: &str) -> SessionRef {
    SessionRef {
        id: id.to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
    }
}

/// A world with a Working Box `demo` launched with "ship it" as session `p-1`; the next launch is `p-2`.
pub(super) fn started() -> (World, Arc<FakeEngine>, BoxId) {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2"))),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &prompt_req("ship it")).unwrap();
    (w, engine, BoxId::new(first.boxed.id))
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

fn live_ref(w: &World, id: BoxId) -> String {
    w.cx.store()
        .unwrap()
        .live_session(id)
        .unwrap()
        .unwrap()
        .session_ref
}

fn exit_of_first_session(w: &World, id: BoxId) -> Option<i32> {
    let first = column_where(Launches::Table, Launches::Exit, Launches::Box, id.get())
        .order_by(Launches::Id, Order::Asc)
        .limit(1)
        .take();
    let reader = w.cx.store().unwrap().reader().unwrap();
    reader.row(first, |r| r.get(0)).unwrap()
}

/// Makes `p-1` read as having no terminal.
pub(super) fn gone(engine: &FakeEngine) {
    engine.set_probes("p-1", agent::gone());
}

fn warning_codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

#[test]
fn a_gone_terminal_relaunches_with_a_prompt() {
    let (w, engine, id) = started();
    gone(&engine);
    let v = run(&w.cx, &prompt_req("ship it")).unwrap();
    assert!(v.agent.launched);
    let second = relaunch_of(&engine);
    assert_eq!(second.prompt.text(), Some("ship it"));
    assert_eq!(second.resume, None);
    assert_eq!(live_ref(&w, id), "p-2");
}

#[test]
fn a_relaunch_records_the_given_parent_session() {
    let (w, engine, id) = started();
    gone(&engine);
    let store = w.cx.store().unwrap();
    let parent = store.open_host_session("/w", None, None).unwrap();
    run(
        &w.cx,
        &StartReq {
            parent: Some(SessionId::new(parent)),
            ..prompt_req("again")
        },
    )
    .unwrap();
    let raw = Conn::open_file(&w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    let newest = column_where(
        Sessions::Table,
        Sessions::ParentSession,
        Sessions::Box,
        id.get(),
    )
    .order_by(Sessions::Ordinal, Order::Desc)
    .limit(1)
    .take();
    let got: Option<i64> = raw.row(newest, |r| r.get(0)).unwrap();
    assert_eq!(got, Some(parent));
}

#[test]
fn an_exited_agent_relaunches_and_the_old_session_keeps_its_exit() {
    let (w, engine, id) = started();
    engine.set_probes("p-1", agent::exited(Some(3)));
    run(&w.cx, &prompt_req("ship it")).unwrap();
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(live_ref(&w, id), "p-2");
    assert_eq!(exit_of_first_session(&w, id), Some(3));
}

#[test]
fn a_given_prompt_relaunches_with_a_fresh_brief_and_is_stored() {
    let (w, engine, id) = started();
    gone(&engine);
    set_agent_session(&w, id, "agent-1");
    run(&w.cx, &prompt_req("new prompt")).unwrap();
    let second = relaunch_of(&engine);
    assert_eq!(second.prompt.text(), Some("new prompt"));
    assert_eq!(second.resume, None);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.prompt.as_deref(), Some("new prompt"));
}

#[test]
fn a_stored_agent_session_relaunches_with_resume() {
    let (w, engine, id) = started();
    gone(&engine);
    set_agent_session(&w, id, "agent-1");
    run(&w.cx, &resume_req()).unwrap();
    let second = relaunch_of(&engine);
    assert_eq!(second.resume.as_deref(), Some("agent-1"));
    assert_eq!(live_ref(&w, id), "p-2");
}

#[test]
fn a_stored_agent_session_alone_relaunches_with_resume() {
    let (w, engine, id) = started();
    gone(&engine);
    set_agent_session(&w, id, "agent-1");
    assert!(run(&w.cx, &req("demo")).unwrap().agent.launched);
    assert_eq!(relaunch_of(&engine).resume.as_deref(), Some("agent-1"));
}

#[test]
fn an_unread_agent_counts_as_live() {
    let (w, engine, id) = started();
    engine.set_probes("p-1", Probe::TimedOut);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(live_ref(&w, id), "p-1");
}

#[test]
fn a_given_prompt_with_a_live_agent_warns_and_changes_nothing() {
    let (w, engine, id) = started();
    w.cx.take_warnings();
    run(&w.cx, &prompt_req("new prompt")).unwrap();
    let warned = w.cx.take_warnings();
    let msg = warned.iter().find(|x| x.code == "prompt_ignored").unwrap();
    assert_eq!(
        msg.message,
        "--prompt ignored: the agent for demo is live; use moor steer"
    );
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(live_ref(&w, id), "p-1");
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.prompt.as_deref(), Some("ship it"));
}

#[test]
fn a_live_agent_without_a_prompt_does_not_warn() {
    let (w, _engine, _id) = started();
    w.cx.take_warnings();
    run(&w.cx, &req("demo")).unwrap();
    assert!(!warning_codes(&w).contains(&"prompt_ignored".to_owned()));
}

#[test]
fn the_relaunched_event_carries_the_prompt_and_its_cause() {
    let (w, engine, id) = started();
    gone(&engine);
    crate::ops::start::revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    let e = events.iter().find(|e| e.kind == "relaunched").unwrap();
    assert_eq!(
        e.payload,
        serde_json::json!({"old_pane": null, "new_pane": null, "prompt": "ship it", "cause": "wake"})
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
    w.cx = w.cx.with_engine(engine.clone());
    let id = BoxId::new(run(&w.cx, &prompt_req("ship it")).unwrap().boxed.id);
    gone(&engine);
    let v = run(&w.cx, &prompt_req("ship it")).unwrap();
    assert!(!v.agent.launched);
    let store = w.cx.store().unwrap();
    assert!(store.box_row(id).unwrap().unwrap().launch_failure.is_some());
    let events = store.events(id, 100).unwrap();
    assert!(events.iter().all(|e| e.kind != "relaunched"));
}

#[test]
fn a_gone_terminal_with_no_prompt_and_no_agent_session_launches_with_no_prompt() {
    let (w, engine, id) = started();
    let clear = Fields {
        prompt: Field::Clear,
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(clear))
        .unwrap();
    gone(&engine);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(relaunch_of(&engine).prompt, LaunchPrompt::None);
}

#[test]
fn a_gone_terminal_start_is_refused_while_another_op_holds_the_lock() {
    let (w, engine, _id) = started();
    gone(&engine);
    let held = super::hold(&w, "close");
    let err = run(&w.cx, &prompt_req("ship it")).unwrap_err();
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert_eq!(engine.launches().len(), 1);
    drop(held);
    assert!(run(&w.cx, &prompt_req("ship it")).unwrap().agent.launched);
}

#[test]
fn a_relaunch_watches_its_new_agent_for_a_first_run_prompt() {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2")))
            .prompt("trust this folder", FirstRunPrompt::FolderTrust)
            .probes(
                "p-2",
                agent::on_screen(
                    agent::read(None, Ts::from_millis(1_000), 0),
                    "Do you trust this folder?",
                ),
            ),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &prompt_req("ship it")).unwrap();
    let id = BoxId::new(first.boxed.id);
    gone(&engine);
    let v = run(&w.cx, &prompt_req("ship it")).unwrap();
    assert!(v.agent.launched);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let blocked = events
        .iter()
        .find(|e| e.kind == "agent_prompt_blocked")
        .unwrap();
    assert_eq!(blocked.payload.get("prompt"), Some(&"folder_trust".into()));
}

/// A world whose Working Box `demo` has no prompt and no session.
fn promptless() -> (World, Arc<FakeEngine>, BoxId) {
    let (w, engine, id) = started();
    let clear = Fields {
        prompt: Field::Clear,
        ..Fields::default()
    };
    let store = w.cx.store().unwrap();
    store.apply(Decision::new(id).set(clear)).unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.end_session(live.id, None, None).unwrap();
    (w, engine, id)
}

#[test]
fn a_prompt_on_a_promptless_box_without_a_session_is_stored_and_launches_once() {
    let (w, engine, id) = promptless();
    let v = run(&w.cx, &prompt_req("late prompt")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(relaunch_of(&engine).prompt.text(), Some("late prompt"));
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.prompt.as_deref(), Some("late prompt"));
}

#[test]
fn a_second_start_without_a_prompt_does_not_launch_again() {
    let (w, engine, _id) = promptless();
    run(&w.cx, &prompt_req("late prompt")).unwrap();
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
}

#[test]
fn an_issue_alone_launches_a_promptless_box_asking_to_resolve_it() {
    let (w, engine, _id) = promptless();
    let issue = StartReq {
        issue: Some("42".into()),
        ..req("demo")
    };
    let v = run(&w.cx, &issue).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(
        relaunch_of(&engine).prompt.text(),
        Some("Resolve issue 42.")
    );
}

#[test]
fn a_prompt_on_a_promptless_box_with_a_live_agent_warns() {
    let (w, engine, id) = started();
    let clear = Fields {
        prompt: Field::Clear,
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(clear))
        .unwrap();
    w.cx.take_warnings();
    run(&w.cx, &prompt_req("late prompt")).unwrap();
    assert!(warning_codes(&w).contains(&"prompt_ignored".to_owned()));
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_given_prompt_replaces_the_stored_prompt_when_there_is_no_session() {
    let (w, engine, id) = started();
    let store = w.cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.end_session(live.id, None, None).unwrap();
    let v = run(&w.cx, &prompt_req("second prompt")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(relaunch_of(&engine).prompt.text(), Some("second prompt"));
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.prompt.as_deref(), Some("second prompt"));
}

/// A resume request for `demo`.
fn resume_req() -> StartReq {
    StartReq {
        resume: true,
        ..req("demo")
    }
}

#[test]
fn resume_without_a_stored_session_is_refused() {
    let (w, engine, _id) = started();
    gone(&engine);
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
fn resume_of_a_box_that_does_not_exist_is_refused() {
    let w = world(FakeDriver::new());
    let e = run(&w.cx, &resume_req()).unwrap_err();
    assert_eq!(e.error.to_string(), "no agent session to resume");
}

#[test]
fn resume_on_a_live_agent_does_not_launch() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    let v = run(&w.cx, &resume_req()).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn resume_on_a_gone_terminal_launches_the_resume() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&engine);
    let v = run(&w.cx, &resume_req()).unwrap();
    assert!(v.agent.launched);
    assert_eq!(relaunch_of(&engine).resume.as_deref(), Some("agent-1"));
}

/// A world with a Working prompt-less Box `demo`, no session, and stored agent session `agent-1`.
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
    let id = BoxId::new(first.boxed.id);
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
fn a_prompt_beats_resume_when_there_is_no_session_row() {
    let (w, engine, _id) = idle_of_sessions();
    let r = StartReq {
        prompt: Some("G".into()),
        ..resume_req()
    };
    run(&w.cx, &r).unwrap();
    let all = engine.launches();
    let first = all.first().unwrap();
    assert_eq!(first.resume, None);
    assert_eq!(first.prompt.text(), Some("G"));
}

#[test]
fn resume_with_a_prompt_is_not_refused_without_a_stored_session() {
    let (w, engine, id) = idle_of_sessions();
    set_agent_session(&w, id, "");
    let r = StartReq {
        prompt: Some("G".into()),
        ..resume_req()
    };
    let v = run(&w.cx, &r).unwrap();
    assert!(v.agent.launched);
    assert_eq!(
        engine.launches().first().and_then(|l| l.prompt.text()),
        Some("G")
    );
}
