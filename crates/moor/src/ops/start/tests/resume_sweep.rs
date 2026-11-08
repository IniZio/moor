//! A status sweep gives the agent of a Box that stopped mid-turn what its wake could not.

use std::sync::Arc;

use moor_core::activity::Probe;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{AgentFate, EngineError, SessionRef};
use moor_core::{BoxId, BoxName, SteerReq};

use super::relaunch::{gone, prompt_req, session, set_agent_session};
use super::revive::{answering, elapsed_ms, notes, silent, woken_after_suspend, CRITICAL};
use super::{world, World};
use crate::ops::start::run;
use crate::testing::agent;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

/// The agent session reference of `id`'s live session.
fn live_ref(w: &World, id: BoxId) -> String {
    let store = w.cx.store().unwrap();
    store.live_session(id).unwrap().unwrap().session_ref
}

/// From now on every probe of `id`'s live agent reads `p`.
fn probes_read(w: &World, engine: &FakeEngine, id: BoxId, p: Probe) {
    engine.set_probes(&live_ref(w, id), p);
}

/// The agent as a probe finds it running.
fn running_pane() -> Probe {
    agent::working(Ts::from_millis(0))
}

/// Runs one sweep of the open Boxes and returns how long it took.
fn sweep(w: &World) -> i64 {
    let from = w.cx.clock().now().as_millis();
    crate::ops::settle_all(&w.cx, w.cx.store().unwrap(), &|| false).unwrap();
    elapsed_ms(w, from)
}

fn owes(w: &World, id: BoxId) -> bool {
    w.cx.store().unwrap().resume_first(id).unwrap().is_some()
}

fn count_of(w: &World, id: BoxId, kind: &str) -> usize {
    let events = w.cx.store().unwrap().events(id, 200).unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

/// A Box suspended mid-turn whose wake found it silent, its agent's pane since read as `pane`.
fn silent_wake(
    launches: Vec<Result<SessionRef, EngineError>>,
    fates: Vec<Result<AgentFate, EngineError>>,
    pane: Probe,
) -> (World, Arc<FakeEngine>, BoxId) {
    silent_wake_on(FakeEngine::new(), launches, fates, pane)
}

/// As [`silent_wake`], on `engine`.
fn silent_wake_on(
    engine: FakeEngine,
    launches: Vec<Result<SessionRef, EngineError>>,
    fates: Vec<Result<AgentFate, EngineError>>,
    pane: Probe,
) -> (World, Arc<FakeEngine>, BoxId) {
    let mut engine = engine.launch_result(Ok(session("p-1")));
    for l in launches {
        engine = engine.launch_result(l);
    }
    let engine = Arc::new(engine.fates([silent(), fates].concat()));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &prompt_req("ship it")).unwrap();
    let id = BoxId::new(first.boxed.id);
    set_agent_session(&w, id, "agent-1");
    woken_after_suspend(&w, id, CRITICAL);
    assert_eq!(engine.launches().len(), 1, "nothing launched while silent");
    assert!(owes(&w, id));
    probes_read(&w, &engine, id, pane);
    (w, engine, id)
}

#[test]
fn a_box_silent_across_sweeps_costs_one_bounded_probe_each_and_is_never_relaunched() {
    let (w, engine, id) = silent_wake(vec![], vec![], Probe::TimedOut);
    let sref = live_ref(&w, id);
    let before = engine.probe_count(&sref);
    for round in 1..=3 {
        let took = sweep(&w);
        assert!(took <= 5_000, "sweep {round} took {took} ms");
        assert_eq!(
            engine.probe_count(&sref) - before,
            round,
            "one probe in sweep {round}"
        );
    }
    assert_eq!(
        engine.fates_left(),
        0,
        "the sweep asked the Box nothing beyond its probe"
    );
    assert_eq!(engine.launches().len(), 1);
    assert!(notes(&engine).is_empty());
    assert!(owes(&w, id));
}

#[test]
fn an_agent_the_box_says_ended_after_a_silent_wake_is_relaunched_once_and_told_once() {
    let (w, engine, id) = silent_wake(
        vec![Ok(session("p-2"))],
        vec![Ok(AgentFate::Ended { exit: None })],
        agent::exited(None),
    );
    sweep(&w);
    probes_read(&w, &engine, id, running_pane());
    sweep(&w);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(
        engine.launches().get(1).and_then(|l| l.resume.as_deref()),
        Some("agent-1")
    );
    let said = notes(&engine);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said.iter().all(|n| n.contains("processes were lost")),
        "{said:?}"
    );
    assert!(!owes(&w, id));
}

#[test]
fn an_agent_still_running_after_a_silent_wake_is_told_once_that_it_is_intact() {
    let (w, engine, id) = silent_wake(vec![], vec![], running_pane());
    sweep(&w);
    sweep(&w);
    assert_eq!(engine.launches().len(), 1, "no launch over the live agent");
    let said = notes(&engine);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said.iter().all(|n| n.contains("intact")), "{said:?}");
    assert!(!owes(&w, id));
}

#[test]
fn a_relaunch_that_fails_after_a_silent_wake_is_tried_once_and_drops_the_mark() {
    let failed = EngineError::Failed {
        step: "pane",
        message: "no pane".to_owned(),
    };
    let (w, engine, id) = silent_wake(
        vec![Err(failed)],
        vec![Ok(AgentFate::Ended { exit: None })],
        agent::exited(None),
    );
    for _ in 0..3 {
        sweep(&w);
    }
    assert_eq!(engine.launches().len(), 2, "one relaunch attempt");
    assert_eq!(count_of(&w, id, "session_failed"), 1);
    assert!(notes(&engine).is_empty());
    assert!(!owes(&w, id));
}

#[test]
fn an_agent_that_ended_after_a_silent_wake_drops_the_mark_and_settles_as_usual() {
    let (w, engine, id) = silent_wake(vec![], vec![], Probe::TimedOut);
    let store = w.cx.store().unwrap();
    store.end_current_session(id, "exited", Some(0)).unwrap();
    sweep(&w);
    sweep(&w);
    assert!(!owes(&w, id));
    assert!(store.box_row(id).unwrap().unwrap().session.is_quiet());
    assert_eq!(engine.launches().len(), 1);
    let said = notes(&engine);
    assert!(said.is_empty(), "{said:?}");
}

#[test]
fn an_agent_whose_box_a_start_woke_after_a_mid_turn_suspend_is_told_once() {
    let (w, engine, id) = answering(vec![]);
    let store = w.cx.store().unwrap();
    let suspend = BoxLifecycleInput::Suspend {
        reason: CRITICAL.to_owned(),
    };
    store
        .apply(
            Decision::new(id)
                .lifecycle(suspend)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    store.set_resume_first(id, Some(0)).unwrap();
    run(&w.cx, &super::req("demo")).unwrap();
    probes_read(&w, &engine, id, running_pane());
    sweep(&w);
    sweep(&w);
    let said = notes(&engine);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said.iter().all(|n| n.contains("intact")), "{said:?}");
    assert!(!owes(&w, id));
}

/// The notes sent that contain `text`.
fn told(engine: &FakeEngine, text: &str) -> usize {
    notes(engine).iter().filter(|n| n.contains(text)).count()
}

#[test]
fn an_agent_a_steer_relaunched_after_a_silent_wake_is_told_once_that_its_processes_were_lost() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let (w, engine, id) = silent_wake_on(
        engine,
        vec![],
        vec![Ok(AgentFate::Ended { exit: None })],
        agent::idle(Ts::from_millis(0)),
    );
    gone(&w);
    let req = SteerReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        text: Some("go on".to_owned()),
        menu: None,
        interrupt: false,
    };
    crate::ops::steer::run(&w.cx, &req).unwrap();
    assert_eq!(engine.launches().len(), 2, "the steer relaunched the agent");
    probes_read(&w, &engine, id, running_pane());
    sweep(&w);
    sweep(&w);
    assert_eq!(
        told(&engine, "processes were lost"),
        1,
        "{:?}",
        notes(&engine)
    );
    assert_eq!(told(&engine, "intact"), 0, "{:?}", notes(&engine));
    assert!(!owes(&w, id));
}

#[test]
fn an_agent_a_start_relaunched_after_a_silent_wake_is_told_once_that_its_processes_were_lost() {
    let (w, engine, id) = silent_wake(vec![], vec![], Probe::TimedOut);
    gone(&w);
    run(&w.cx, &super::req("demo")).unwrap();
    assert_eq!(engine.launches().len(), 2, "the start relaunched the agent");
    probes_read(&w, &engine, id, running_pane());
    sweep(&w);
    sweep(&w);
    assert_eq!(
        told(&engine, "processes were lost"),
        1,
        "{:?}",
        notes(&engine)
    );
    assert_eq!(told(&engine, "intact"), 0, "{:?}", notes(&engine));
    assert!(!owes(&w, id));
}

#[test]
fn an_agent_that_idled_before_the_sweep_after_a_silent_wake_is_still_told_once() {
    let (w, engine, id) = silent_wake(vec![], vec![], Probe::TimedOut);
    let at = w.cx.clock().now().as_millis() - 1000;
    let idle = agent::idle(Ts::from_millis(at));
    engine.set_probes(&live_ref(&w, id), idle);
    sweep(&w);
    sweep(&w);
    assert_eq!(notes(&engine).len(), 1, "{:?}", notes(&engine));
    assert_eq!(told(&engine, "intact"), 1, "{:?}", notes(&engine));
    assert!(!owes(&w, id));
}
