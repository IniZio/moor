//! Tests for `wait` while a Box's agent launches and before its first turn.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{AgentState, STARTUP_GRACE};
use moor_core::config::Flags;
use moor_core::domain::decision::{Decision, Effect, EffectChange, NewBox, SpecFields};
use moor_core::domain::inbox::InboxKind;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::EngineError;
use moor_core::{BoxId, BoxName, ErrorKind, StartReq};
use moor_schema::tables::Sessions;
use sea_query::{Alias, Table};

use super::{agent_cx, ready, req, set_up, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::ops::start::launch_after_retry;
use crate::ops::wait::run;
use crate::store::db::testing::open_conn;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

/// The session a launch in these tests opens.
const LAUNCHED: &str = "moor-1";

/// Context over `engine`.
fn launch_cx(dir: &std::path::Path, engine: FakeEngine) -> Cx {
    let (cx, _) = super::cx(dir);
    cx.with_engine(Arc::new(engine))
}

#[test]
fn idle_waits_for_an_agent_still_to_launch() {
    let dir = tempfile::tempdir().unwrap();
    let engine = FakeEngine::new().probes(LAUNCHED, agent::idle(Ts::from_millis(0)));
    let cx = launch_cx(dir.path(), engine);
    let id = set_up(&cx, &Flags::default());
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(
        v.timed_out,
        "the Box is ready but its agent has not launched"
    );
    launch_after_retry(&cx, cx.store().unwrap(), id).unwrap();
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out, "the launched agent read idle after a turn");
}

#[test]
fn a_failed_launch_fails_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let engine = FakeEngine::new().launch_result(Err(EngineError::Failed {
        step: "launch",
        message: "no tab".into(),
    }));
    let cx = launch_cx(dir.path(), engine);
    let id = set_up(&cx, &Flags::default());
    launch_after_retry(&cx, cx.store().unwrap(), id).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.effect, None, "the failed launch ended the launch");
    let e = run(&cx, &req(None, Some(5))).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("no tab"), "{}", e.error);
}

/// Inserts Box `demo` with `prompt`, ready with a launched agent on [`SESSION`]. A `carried`
/// prompt was the launch's first prompt; otherwise the agent resumed and was given none.
fn prompted_live(cx: &Cx, prompt: Option<&str>, carried: bool) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
        spec: SpecFields {
            prompt: prompt.map(str::to_owned),
            ..SpecFields::default()
        },
    };
    let store = cx.store().unwrap();
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    ready(cx, id);
    let argv = prompt.filter(|_| carried);
    if let Some(text) = argv {
        store
            .enqueue(id, InboxKind::Prompt, text, "prompt")
            .unwrap();
    }
    store.begin_launch_with(id, SESSION, argv, None).unwrap();
    id
}

#[test]
fn a_fresh_agent_idle_before_its_first_turn_keeps_idle_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    prompted_live(&cx, Some("ship it"), true);
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out, "the first prompt's turn has not shown yet");
}

#[test]
fn a_fresh_agent_reads_starting_until_the_startup_grace_ends() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    let id = prompted_live(&cx, Some("ship it"), true);
    assert_eq!(state(&cx, id), AgentState::Starting);
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out, "the first prompt's turn has not shown yet");
    cx.clock().sleep(STARTUP_GRACE + Duration::from_secs(1));
    assert_eq!(state(&cx, id), AgentState::Idle);
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(
        !v.timed_out,
        "an agent idle at turn 0 past the grace never took its prompt"
    );
}

/// What one read of Box `id`'s agent finds it doing.
fn state(cx: &Cx, id: BoxId) -> AgentState {
    let store = cx.store().unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    let now = cx.clock().now();
    let reading = activity::read(cx, store, &row, now, activity::PROBE_DEADLINE).unwrap();
    reading.activity.unwrap().state
}

#[test]
fn a_resumed_agent_idle_before_any_turn_on_a_prompted_box_is_idle_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    let id = prompted_live(&cx, Some("ship it"), false);
    assert_eq!(state(&cx, id), AgentState::Idle);
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out, "this launch was given no prompt to take up");
    assert_eq!(v.waited_ms, 0);
}

#[test]
fn a_fresh_agent_started_for_an_issue_keeps_idle_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let cx = launch_cx(dir.path(), FakeEngine::new().probes(LAUNCHED, fresh));
    let start = StartReq {
        issue: Some("#7".into()),
        no_agent: false,
        ..agentless_start()
    };
    crate::ops::start::run(&cx, &start).unwrap();
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out, "the issue's turn has not shown yet");
}

#[test]
fn a_box_left_launching_with_no_machine_ends_the_launch_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let cx = launch_cx(dir.path(), FakeEngine::new());
    let store = cx.store().unwrap();
    let id = super::add(&cx);
    let launching = Decision::new(id).effect(EffectChange::Next(Effect::Launch));
    store.apply(launching).unwrap();
    resume_setup(&cx, store, id).unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
}

#[test]
fn a_fresh_agent_with_no_prompt_is_idle_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    prompted_live(&cx, None, false);
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.waited_ms, 0);
}

#[test]
fn an_agent_still_to_launch_when_its_process_died_launches_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let engine = FakeEngine::new().probes(LAUNCHED, agent::gone());
    let engine = Arc::new(engine);
    let (cx, _) = super::cx(dir.path());
    let cx = cx.with_engine(engine.clone());
    let id = set_up(&cx, &Flags::default());
    let store = cx.store().unwrap();
    assert_eq!(fail_interrupted(&cx, store).unwrap(), [id]);
    resume_setup(&cx, store, id).unwrap();
    assert_eq!(engine.launches().len(), 1);
    assert!(store.live_launch(id).unwrap().is_some());
    assert!(fail_interrupted(&cx, store).unwrap().is_empty());
}

#[test]
fn an_agentless_start_of_a_box_left_launching_ends_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let cx = launch_cx(dir.path(), FakeEngine::new());
    let id = set_up(&cx, &Flags::default());
    let start = agentless_start();
    crate::ops::start::run(&cx, &start).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.effect, None);
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out);
}

/// An agentless start of Box `demo`.
fn agentless_start() -> StartReq {
    StartReq {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: true,
        cold: false,
        resume: false,
        repo: None,
        parent: None,
        also: None,
        identity: None,
    }
}

/// Context with no agent running, whose every launch fails with `no tab`.
fn failing_cx(dir: &std::path::Path) -> Cx {
    let engine = FakeEngine::new()
        .probes(LAUNCHED, agent::gone())
        .launch_result(Err(EngineError::Failed {
            step: "launch",
            message: "no tab".into(),
        }));
    launch_cx(dir, engine)
}

#[test]
fn a_launch_that_fails_after_a_restart_ends_the_launch_and_fails_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let cx = failing_cx(dir.path());
    let id = set_up(&cx, &Flags::default());
    let store = cx.store().unwrap();
    resume_setup(&cx, store, id).unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
    let e = run(&cx, &req(None, Some(5))).unwrap_err();
    assert!(e.error.to_string().contains("no tab"), "{}", e.error);
}

#[test]
fn a_seeded_launch_that_fails_ends_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let cx = failing_cx(dir.path());
    let id = set_up(&cx, &Flags::default());
    let store = cx.store().unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    let view = crate::ops::start::begin_seeded(&cx, store, &row, "go".into()).unwrap();
    assert!(!view.launched);
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
}

#[test]
fn a_relaunch_after_a_retry_that_errors_ends_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let cx = launch_cx(dir.path(), FakeEngine::new());
    let id = set_up(&cx, &Flags::default());
    let store = cx.store().unwrap();
    let db = open_conn(&dir.path().join("moor.db"));
    let gone = Alias::new("sessions_gone");
    let hide = Table::rename().table(Sessions::Table, gone.clone()).take();
    db.schema(&hide).unwrap();
    launch_after_retry(&cx, store, id).unwrap_err();
    let back = Table::rename().table(gone, Sessions::Table).take();
    db.schema(&back).unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
}

#[test]
fn an_issue_given_to_a_box_with_no_agent_is_its_launch_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let cx = launch_cx(dir.path(), FakeEngine::new().probes(LAUNCHED, fresh));
    crate::ops::start::run(&cx, &agentless_start()).unwrap();
    let start = StartReq {
        issue: Some("#7".into()),
        no_agent: false,
        ..agentless_start()
    };
    crate::ops::start::run(&cx, &start).unwrap();
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out, "the issue's turn has not shown yet");
}
