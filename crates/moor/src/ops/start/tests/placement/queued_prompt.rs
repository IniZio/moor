//! A prompt given to a start that has to queue reaches the agent once the queue lets it go.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{AgentFate, EngineError, Steer};
use moor_core::StartReq;

use super::pressure::{tick, wake_up};
use super::*;
use crate::ops::start::tests::relaunch::{gone, session, set_agent_session};
use crate::testing::engine::FakeEngine;

fn with_prompt(name: &str, prompt: &str) -> StartReq {
    StartReq {
        prompt: Some(prompt.into()),
        ..req(name)
    }
}

fn texts(engine: &FakeEngine) -> Vec<String> {
    engine
        .steers()
        .into_iter()
        .filter_map(|(_, s)| match s {
            Steer::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// The agent's new session reports Idle.
fn reports_idle(engine: FakeEngine) -> FakeEngine {
    engine.probes(
        "p-2",
        crate::testing::agent::idle(Ts::from_millis(i64::MAX / 2)),
    )
}

fn sweep(w: &World) {
    start::settle_all(&w.cx, w.cx.store().unwrap(), &|| false).unwrap();
}

fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 200).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn steered(w: &World, id: BoxId) -> usize {
    kinds(w, id).iter().filter(|k| *k == "steered").count()
}

fn engine_world(engine: FakeEngine) -> (World, i64, Arc<FakeEngine>) {
    let (mut w, hv) = live_world(32_768);
    let engine = Arc::new(engine);
    w.cx = w.cx.with_engine(engine.clone());
    (w, hv, engine)
}

/// A Box with a live agent session, asleep, then started with `prompt` while short of room.
fn queued_with_prompt(engine: FakeEngine, prompt: &str) -> (World, i64, Arc<FakeEngine>, BoxId) {
    let (w, hv, engine) = engine_world(engine);
    let id = BoxId::new(
        start::run(&w.cx, &with_prompt("a", "first"))
            .unwrap()
            .boxed
            .id,
    );
    set_agent_session(&w, id, "agent-1");
    put_to_sleep(&w, id);
    tick(&w, hv, SHORT, 0, 0);
    let view = start::run(&w.cx, &with_prompt("a", prompt)).unwrap();
    assert!(view.queued);
    (w, hv, engine, id)
}

#[test]
fn a_new_start_that_queued_launches_with_its_prompt_when_promoted() {
    let (w, hv, engine) = engine_world(FakeEngine::new());
    report(&w, hv, SHORT, None);
    let view = start::run(&w.cx, &with_prompt("a", "ship it")).unwrap();
    assert!(view.queued);
    assert!(engine.launches().is_empty());
    tick(&w, hv, 32_768, 0, 0);
    let all = engine.launches();
    assert_eq!(all.len(), 1);
    assert_eq!(all.first().and_then(|l| l.prompt.text()), Some("ship it"));
}

#[test]
fn a_new_start_on_an_issue_that_queued_launches_on_that_issue_when_promoted() {
    let (w, hv, engine) = engine_world(FakeEngine::new());
    report(&w, hv, SHORT, None);
    let asked = StartReq {
        issue: Some("#7".into()),
        ..req("a")
    };
    assert!(start::run(&w.cx, &asked).unwrap().queued);
    tick(&w, hv, 32_768, 0, 0);
    let all = engine.launches();
    assert_eq!(
        all.first().and_then(|l| l.prompt.text()),
        Some("Resolve issue #7.")
    );
}

#[test]
fn a_prompt_held_for_an_asleep_box_reaches_a_reattached_agent_once() {
    let engine = FakeEngine::new()
        .launch_result(Ok(session("p-1")))
        .fates(vec![Ok(AgentFate::Running)]);
    let (w, hv, engine, id) = queued_with_prompt(engine, "carry on");
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(
        lifecycle(&w, id),
        moor_core::domain::box_state::BoxLifecycle::Ready
    );
    assert_eq!(engine.launches().len(), 1, "no new agent");
    assert_eq!(texts(&engine), ["carry on"]);
    put_to_sleep(&w, id);
    wake_up(&w, id).unwrap();
    assert_eq!(texts(&engine), ["carry on"], "not sent again");
}

#[test]
fn a_prompt_held_for_an_asleep_box_reaches_a_relaunched_agent_once_it_reports_idle() {
    let engine = reports_idle(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2"))),
    );
    let (w, hv, engine, id) = queued_with_prompt(engine, "carry on");
    gone(&w);
    tick(&w, hv, 32_768, 0, 0);
    let all = engine.launches();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all.get(1).and_then(|l| l.resume.as_deref()),
        Some("agent-1")
    );
    sweep(&w);
    sweep(&w);
    assert_eq!(texts(&engine), ["carry on"]);
    assert_eq!(steered(&w, id), 1);
}

#[test]
fn a_held_prompt_whose_steer_fails_once_is_given_on_a_later_sweep_exactly_once() {
    let engine = reports_idle(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2")))
            .steer_result(Err(EngineError::Failed {
                step: "type",
                message: "pane busy".into(),
            })),
    );
    let (w, hv, engine, id) = queued_with_prompt(engine, "carry on");
    gone(&w);
    tick(&w, hv, 32_768, 0, 0);
    sweep(&w);
    assert!(kinds(&w, id).contains(&"prompt_not_given".to_owned()));
    assert_eq!(steered(&w, id), 0);
    sweep(&w);
    sweep(&w);
    assert_eq!(steered(&w, id), 1, "given once, without another wake");
    assert_eq!(
        texts(&engine),
        ["carry on", "carry on"],
        "one failed try, one given"
    );
}

#[test]
fn a_held_prompt_stops_being_tried_after_its_failed_tries() {
    let fail = || {
        Err(EngineError::Failed {
            step: "type",
            message: "pane busy".into(),
        })
    };
    let mut engine = FakeEngine::new()
        .launch_result(Ok(session("p-1")))
        .launch_result(Ok(session("p-2")));
    for _ in 0..5 {
        engine = engine.steer_result(fail());
    }
    let (w, hv, engine, id) = queued_with_prompt(reports_idle(engine), "carry on");
    gone(&w);
    tick(&w, hv, 32_768, 0, 0);
    for _ in 0..5 {
        sweep(&w);
    }
    let failed = kinds(&w, id)
        .iter()
        .filter(|k| *k == "prompt_not_given")
        .count();
    assert_eq!(failed, 3);
    assert_eq!(texts(&engine).len(), 3);
}

#[test]
fn a_relaunch_that_starts_on_the_stored_prompt_is_not_told_it_again() {
    let engine = FakeEngine::new()
        .launch_result(Ok(session("p-1")))
        .launch_result(Ok(session("p-2")));
    let (w, hv, engine, id) = queued_with_prompt(engine, "carry on");
    apply(
        &w,
        Decision::new(id).set(Fields {
            agent_session_id: Field::Clear,
            ..Fields::default()
        }),
    );
    gone(&w);
    tick(&w, hv, 32_768, 0, 0);
    let all = engine.launches();
    assert_eq!(all.get(1).and_then(|l| l.prompt.text()), Some("carry on"));
    assert_eq!(texts(&engine), Vec::<String>::new());
}
