//! A text steer waits in the Box's inbox from just before its send until its turn ends, so a
//! crash mid-send leaves it in doubt rather than lost, and a refused steer leaves nothing queued.

use std::panic::{catch_unwind, AssertUnwindSafe};

use moor_core::activity::Probe;
use moor_core::domain::inbox::{Delivery, InboxKind};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{EngineError, Steer};

use super::{blocked, fx_with, steered, text, Fx};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::steer::{run, tell};
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

/// Each sent row's text.
fn sent(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().sent(f.id).unwrap();
    rows.into_iter().map(|r| r.text).collect()
}

/// The payloads of the Box's `turn_done` events, oldest first.
fn turns_done(f: &Fx) -> Vec<serde_json::Value> {
    let all = f.cx.store().unwrap().events(f.id, 100).unwrap();
    all.into_iter()
        .filter(|e| e.kind == "turn_done")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_steer_to_an_awake_box_is_delivered_and_its_row_goes_at_turn_done() {
    let engine = FakeEngine::new()
        .probes("agent-1", idle_after(0))
        .probes_after_steer("agent-1", agent::at_turn(super::working(), 0));
    let f = fx_with(engine, true);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(
        f.engine.steers(),
        [("agent-1".to_owned(), Steer::Text("hi".to_owned()))]
    );
    assert_eq!(sent(&f), ["hi"], "the row is the open turn");
    let inbox = steered(&f).first().unwrap()["inbox"].clone();
    settle_once(&f);
    assert_eq!(sent(&f), ["hi"], "a working agent keeps the turn open");
    f.engine.set_probes("agent-1", idle_after(1));
    settle_once(&f);
    assert!(sent(&f).is_empty(), "the row goes with its turn");
    assert_eq!(
        turns_done(&f),
        [serde_json::json!({"steer": v.steer, "outcome": "idle", "inbox": inbox})]
    );
}

#[test]
#[expect(clippy::panic, reason = "the panic stands in for the controller dying")]
fn a_crash_after_the_mark_leaves_the_row_in_doubt() {
    let f = fx_with(FakeEngine::new().probes("agent-1", idle_after(0)), true);
    f.engine.on_steer(|| panic!("the controller dies mid-send"));
    let crashed = catch_unwind(AssertUnwindSafe(|| drop(run(&f.cx, &text("hi")))));
    assert!(crashed.is_err());
    let store = f.cx.store().unwrap();
    let row = store.next_for(f.id).unwrap().unwrap();
    assert_eq!(row.text, "hi");
    assert!(
        matches!(row.delivery, Delivery::InDoubt { attempts: 1, .. }),
        "{:?}",
        row.delivery
    );
    assert!(steered(&f).is_empty(), "the send was never confirmed");
}

#[test]
fn a_turn_that_answers_the_newest_steer_absorbs_the_older_one() {
    let f = fx_with(FakeEngine::new().probes("agent-1", idle_after(0)), true);
    run(&f.cx, &text("one")).unwrap();
    let two = run(&f.cx, &text("two")).unwrap();
    let texts: Vec<_> = f.engine.steers().into_iter().map(|s| s.1).collect();
    assert_eq!(
        texts,
        [Steer::Text("one".to_owned()), Steer::Text("two".to_owned())]
    );
    assert_eq!(sent(&f), ["one", "two"]);
    f.engine.set_probes("agent-1", idle_after(1));
    settle_once(&f);
    let answered: Vec<_> = turns_done(&f).iter().map(|d| d["steer"].clone()).collect();
    assert_eq!(
        answered,
        [two.steer.unwrap()],
        "one turn answered both, as one prompt"
    );
    assert!(sent(&f).is_empty(), "the older row went with it");
}

#[test]
fn sends_a_gone_agent_refused_leave_no_row_and_a_later_steer_is_delivered() {
    let engine = FakeEngine::new()
        .probes("agent-1", idle_after(0))
        .steer_result(Err(EngineError::AgentGone))
        .steer_result(Err(EngineError::AgentGone))
        .steer_result(Err(EngineError::AgentGone));
    let f = fx_with(engine, true);
    run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(f.engine.steers().len(), 3, "each refused send is retried");
    assert_eq!(f.cx.store().unwrap().next_for(f.id).unwrap(), None);
    run(&f.cx, &text("later")).unwrap();
    assert_eq!(
        f.engine.steers().last().map(|s| s.1.clone()),
        Some(Steer::Text("later".to_owned()))
    );
    assert_eq!(sent(&f), ["later"]);
}

#[test]
fn a_steer_refused_after_a_refused_send_leaves_no_row() {
    let engine = FakeEngine::new()
        .probes("agent-1", idle_after(0))
        .steer_result(Err(EngineError::AgentGone))
        .probes_after_steer("agent-1", blocked("Allow?"));
    let f = fx_with(engine, true);
    run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(f.engine.steers().len(), 1, "the refused send made a row");
    assert_eq!(f.cx.store().unwrap().next_for(f.id).unwrap(), None);
}

#[test]
fn a_text_that_becomes_a_menu_choice_leaves_only_the_choice() {
    let asking = agent::asking("Which color?", &["red", "blue"], Ts::from_millis(0));
    let engine = FakeEngine::new()
        .probes("agent-1", idle_after(0))
        .steer_result(Err(EngineError::AgentGone))
        .probes_after_steer("agent-1", asking);
    let f = fx_with(engine, true);
    run(&f.cx, &text("blue")).unwrap();
    let sends: Vec<_> = f.engine.steers().into_iter().map(|s| s.1).collect();
    assert_eq!(sends, [Steer::Text("blue".to_owned()), Steer::Menu(2)]);
    let rows = f.cx.store().unwrap().sent(f.id).unwrap();
    let rows: Vec<_> = rows.into_iter().map(|r| (r.kind, r.text)).collect();
    assert_eq!(rows, [(InboxKind::Choice, "2".to_owned())]);
    assert_eq!(
        f.cx.store()
            .unwrap()
            .next_for(f.id)
            .unwrap()
            .map(|r| r.text),
        Some("2".to_owned())
    );
}

#[test]
fn a_note_the_agent_refused_leaves_no_row() {
    let engine = FakeEngine::new()
        .probes("agent-1", idle_after(0))
        .steer_result(Err(EngineError::AgentGone));
    let f = fx_with(engine, true);
    let store = f.cx.store().unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    tell(&f.cx, store, &row, "note".to_owned()).unwrap_err();
    assert_eq!(f.engine.steers().len(), 1, "the refused send made a row");
    assert_eq!(store.next_for(f.id).unwrap(), None);
}
