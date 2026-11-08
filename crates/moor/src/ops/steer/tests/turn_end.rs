//! A steer's turn is answered by the first ended turn past the count the steer saw, even one the
//! agent ended while the steer was still being sent, and never by the end of a turn already
//! running.

use moor_core::activity::Probe;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;

use super::{fx_with, text, working, Fx};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::steer::run;
use crate::store::EventRow;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

/// An idle agent after `turn` ended turns.
fn idle_after(turn: u64) -> Probe {
    agent::at_turn(agent::idle(Ts::from_millis(0)), turn)
}

fn settle_once(f: &Fx) {
    let store = f.cx.store().unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    let reading = read(&f.cx, store, &row, f.cx.clock().now(), PROBE_DEADLINE).unwrap();
    settle(&f.cx, store, &row, &reading).unwrap();
}

fn turns_done(f: &Fx) -> Vec<EventRow> {
    let all = f.cx.store().unwrap().events(f.id, 100).unwrap();
    all.into_iter().filter(|e| e.kind == "turn_done").collect()
}

#[test]
fn a_turn_that_ends_before_the_steer_is_recorded_is_answered() {
    let engine = FakeEngine::new()
        .probes("agent-1", idle_after(0))
        .probes_after_steer("agent-1", idle_after(1));
    let f = fx_with(engine, true);
    let v = run(&f.cx, &text("hi")).unwrap();
    settle_once(&f);
    settle_once(&f);
    let done = turns_done(&f);
    assert_eq!(done.len(), 1, "the fast turn is answered exactly once");
    assert_eq!(
        done.first().unwrap().payload,
        serde_json::json!({"steer": v.steer, "outcome": "idle"})
    );
}

#[test]
fn the_end_of_the_turn_running_when_steered_does_not_answer_it() {
    let engine = FakeEngine::new()
        .probes("agent-1", agent::at_turn(working(), 5))
        .probes_after_steer("agent-1", idle_after(6));
    let f = fx_with(engine, true);
    run(&f.cx, &text("next")).unwrap();
    settle_once(&f);
    assert!(
        turns_done(&f).is_empty(),
        "the earlier turn's stop is not this steer's"
    );
    f.engine.set_probes("agent-1", idle_after(7));
    settle_once(&f);
    assert_eq!(
        turns_done(&f).len(),
        1,
        "the steer's own turn end answers it"
    );
}

#[test]
fn a_steer_whose_agent_was_never_read_is_refused_unsent() {
    let f = fx_with(FakeEngine::new().probes("agent-1", Probe::TimedOut), true);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert!(
        err.error.to_string().contains("could not read the agent"),
        "with no turn count to place the turn after, a stale idle report could answer it: {err:?}"
    );
    assert!(f.engine.steers().is_empty(), "nothing was typed");
}

#[test]
fn a_steer_to_an_idle_agent_opens_its_turn() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    let quiet = Decision::new(f.id).session(SessionInput::AgentQuiet);
    f.cx.store().unwrap().apply(quiet).unwrap();
    let row = || f.cx.store().unwrap().box_row(f.id).unwrap().unwrap();
    assert!(row().session.is_quiet());
    run(&f.cx, &text("do x")).unwrap();
    assert!(!row().session.is_quiet());
}
