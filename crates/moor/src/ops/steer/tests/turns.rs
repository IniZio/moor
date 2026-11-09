//! Every steer that reaches the agent, a menu choice, an interrupt and `/exit` as much as a text,
//! opens a turn that its `turn_done` ends once.

use moor_core::seam::clock::Ts;

use super::{fx_with, interrupt, menu, steered, text, Fx};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::steer::run;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

fn settle_once(f: &Fx) {
    let store = f.cx.store().unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    let reading = read(&f.cx, store, &row, f.cx.clock().now(), PROBE_DEADLINE).unwrap();
    settle(&f.cx, store, &row, &reading).unwrap();
}

/// The payloads of the Box's `turn_done` events, oldest first.
pub(super) fn turns_done(f: &Fx) -> Vec<serde_json::Value> {
    let all = f.cx.store().unwrap().events(f.id, 100).unwrap();
    all.into_iter()
        .filter(|e| e.kind == "turn_done")
        .map(|e| e.payload)
        .collect()
}

/// The one `turn_done` that answers the only steer, `steer`, with `outcome`.
pub(super) fn answered(f: &Fx, steer: i64, outcome: &str) -> serde_json::Value {
    let inbox = steered(f).first().unwrap()["inbox"].clone();
    assert!(inbox.is_i64(), "the steer was sent from a row: {inbox}");
    serde_json::json!({"steer": steer, "outcome": outcome, "inbox": inbox})
}

#[test]
fn a_menu_choice_opens_a_turn_its_turn_done_ends() {
    let asking = agent::at_turn(agent::permission("Allow?", Ts::from_millis(1_000)), 5);
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    let v = run(&f.cx, &menu(1)).unwrap();
    assert!(turns_done(&f).is_empty());
    let after = agent::at_turn(agent::idle(Ts::from_millis(2_000)), 6);
    f.engine.set_probes("agent-1", after);
    settle_once(&f);
    settle_once(&f);
    assert_eq!(turns_done(&f), [answered(&f, v.steer, "idle")]);
    assert!(f.cx.store().unwrap().sent(f.id).unwrap().is_empty());
}

#[test]
fn an_interrupt_to_a_working_agent_opens_a_turn_its_turn_done_ends() {
    let f = fx_with(
        FakeEngine::new().probes("agent-1", agent::at_turn(super::working(), 3)),
        true,
    );
    let v = run(&f.cx, &interrupt()).unwrap();
    settle_once(&f);
    assert!(turns_done(&f).is_empty(), "the agent still works");
    let after = agent::at_turn(agent::idle(Ts::from_millis(2_000)), 4);
    f.engine.set_probes("agent-1", after);
    settle_once(&f);
    assert_eq!(turns_done(&f), [answered(&f, v.steer, "idle")]);
}

#[test]
fn an_exit_ends_its_turn_as_exited() {
    let idle = agent::at_turn(agent::idle(Ts::from_millis(0)), 2);
    let f = fx_with(FakeEngine::new().probes("agent-1", idle), true);
    let v = run(&f.cx, &text("/exit")).unwrap();
    f.engine.set_probes("agent-1", agent::exited(Some(0)));
    settle_once(&f);
    assert_eq!(turns_done(&f), [answered(&f, v.steer, "exited")]);
}
