//! Tests for `wait` while a Box's agent launches and before its first turn.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::STARTUP_GRACE;
use moor_core::config::Flags;
use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::EngineError;
use moor_core::{BoxId, BoxName, ErrorKind, StartReq};

use super::{agent_cx, live, req, set_up};
use crate::cx::Cx;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::ops::start::launch_after_retry;
use crate::ops::wait::run;
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
    let e = run(&cx, &req(None, Some(5))).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("no tab"), "{}", e.error);
}

/// Inserts Box `demo` with `prompt`, ready with a launched agent on [`SESSION`].
fn prompted_live(cx: &Cx, prompt: Option<&str>) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
        spec: SpecFields {
            prompt: prompt.map(str::to_owned),
            ..SpecFields::default()
        },
    };
    let id = cx
        .store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .box_id;
    live(cx, id)
}

#[test]
fn a_fresh_agent_idle_before_its_first_turn_keeps_idle_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    prompted_live(&cx, Some("ship it"));
    let v = run(&cx, &req(None, Some(1))).unwrap();
    assert!(v.timed_out, "the first prompt's turn has not shown yet");
}

#[test]
fn a_fresh_agent_idle_past_the_startup_grace_is_idle() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    prompted_live(&cx, Some("ship it"));
    cx.clock().sleep(STARTUP_GRACE + Duration::from_secs(1));
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out);
}

#[test]
fn a_fresh_agent_with_no_prompt_is_idle_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = agent::at_turn(agent::idle(Ts::from_millis(0)), 0);
    let (cx, _) = agent_cx(dir.path(), fresh);
    prompted_live(&cx, None);
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
    let start = StartReq {
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
    };
    crate::ops::start::run(&cx, &start).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.intent, None);
    let v = run(&cx, &req(None, Some(5))).unwrap();
    assert!(!v.timed_out);
}
