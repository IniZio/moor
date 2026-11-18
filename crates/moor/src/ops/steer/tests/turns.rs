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
    assert_eq!(turns_done(&f), [answered(&f, v.steer.unwrap(), "idle")]);
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
    assert_eq!(turns_done(&f), [answered(&f, v.steer.unwrap(), "idle")]);
}

#[test]
fn an_exit_ends_its_turn_as_exited() {
    let idle = agent::at_turn(agent::idle(Ts::from_millis(0)), 2);
    let f = fx_with(FakeEngine::new().probes("agent-1", idle), true);
    let v = run(&f.cx, &text("/exit")).unwrap();
    f.engine.set_probes("agent-1", agent::exited(Some(0)));
    settle_once(&f);
    assert_eq!(turns_done(&f), [answered(&f, v.steer.unwrap(), "exited")]);
}

/// The `steer` ids of the Box's `turn_done` events, sorted.
fn answered_steers(f: &Fx) -> Vec<i64> {
    let mut ids: Vec<i64> = turns_done(f)
        .iter()
        .map(|d| d["steer"].as_i64().unwrap())
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn two_steers_queued_while_busy_are_both_closed_by_the_following_idle() {
    let f = fx_with(
        FakeEngine::new().probes("agent-1", agent::at_turn(super::working(), 0)),
        true,
    );
    let one = run(&f.cx, &text("one")).unwrap().steer.unwrap();
    let two = run(&f.cx, &text("two")).unwrap().steer.unwrap();
    settle_once(&f);
    assert!(turns_done(&f).is_empty(), "the agent still works");
    let after = agent::at_turn(agent::idle(Ts::from_millis(2_000)), 2);
    f.engine.set_probes("agent-1", after);
    settle_once(&f);
    assert_eq!(
        answered_steers(&f),
        [one, two],
        "each steer gets its turn_done"
    );
    assert!(f.cx.store().unwrap().sent(f.id).unwrap().is_empty());
}

/// Claude Code takes a message typed while it works into the turn it is in, so the steers are
/// done when that one turn ends: the agent's ended-turn count goes up once, not once per steer.
#[test]
fn a_steer_the_agent_took_into_its_running_turn_is_closed_when_that_turn_ends() {
    use std::time::Duration;

    let idle = |turn| agent::at_turn(agent::idle(Ts::from_millis(1_000)), turn);
    let f = fx_with(FakeEngine::new().probes("agent-1", idle(1)), true);
    let one = run(&f.cx, &text("one")).unwrap().steer.unwrap();
    f.engine
        .set_probes("agent-1", agent::at_turn(super::working(), 1));
    let two = run(&f.cx, &text("two")).unwrap().steer.unwrap();
    f.cx.clock().sleep(Duration::from_secs(40));
    let ended = f.cx.clock().now();
    f.engine
        .set_probes("agent-1", agent::at_turn(agent::idle(ended), 2));
    settle_once(&f);
    assert!(
        turns_done(&f).is_empty(),
        "right at the turn's end a message left over could still start a turn"
    );
    f.cx.clock().sleep(Duration::from_secs(30));
    settle_once(&f);
    assert_eq!(answered_steers(&f), [one, two]);
    assert!(f.cx.store().unwrap().sent(f.id).unwrap().is_empty());
}

#[test]
fn a_steer_left_for_the_next_turn_waits_for_that_turn() {
    use std::time::Duration;

    let f = fx_with(
        FakeEngine::new().probes("agent-1", agent::at_turn(super::working(), 1)),
        true,
    );
    run(&f.cx, &text("two")).unwrap();
    f.cx.clock().sleep(Duration::from_secs(30));
    let now = f.cx.clock().now();
    f.engine
        .set_probes("agent-1", agent::at_turn(agent::working(now), 2));
    settle_once(&f);
    assert!(turns_done(&f).is_empty(), "the next turn runs the steer");
}

#[test]
fn a_steer_sent_after_a_newer_row_is_not_closed_by_that_row_s_turn() {
    use moor_core::domain::inbox::{InboxKind, SendMark};
    use std::time::Duration;

    let f = fx_with(
        FakeEngine::new().probes("agent-1", agent::at_turn(super::working(), 0)),
        true,
    );
    let store = f.cx.store().unwrap();
    let launch = store.live_launch(f.id).unwrap().unwrap();
    let older = store.enqueue(f.id, InboxKind::Steer, "older", "u").unwrap();
    let newer = store.enqueue(f.id, InboxKind::Steer, "newer", "u").unwrap();
    let send = |row, turn_before| {
        let _turn = f.cx.turn(f.id, crate::cx::owners::Cmd::Steer).unwrap();
        store.mark_sending(f.id, row, launch.id).unwrap();
        let mark = SendMark {
            turn_before: Some(turn_before),
            status_before: None,
        };
        let event = moor_core::domain::event::Event::Steered(moor_core::domain::event::SteerSent {
            session: "agent-1".to_owned(),
            message: "m".to_owned(),
            submit_retried: false,
            turn_before: mark.turn_before,
            status_before: None,
            inbox: Some(row.get()),
        });
        store.confirm_sent(f.id, row, mark, &event).unwrap().1
    };
    // The newer row goes first, as a launch's own prompt does; the older one is typed while
    // the agent works on it.
    let first = send(newer, 0);
    f.cx.clock().sleep(Duration::from_secs(10));
    let second = send(older, 1);
    f.engine.set_probes(
        "agent-1",
        agent::at_turn(agent::idle(Ts::from_millis(2_000)), 1),
    );
    settle_once(&f);
    assert!(
        turns_done(&f).is_empty(),
        "the turn the first send started ended, not the one the second waits on"
    );
    f.engine.set_probes(
        "agent-1",
        agent::at_turn(agent::idle(Ts::from_millis(3_000)), 2),
    );
    settle_once(&f);
    let mut both = [first, second];
    both.sort_unstable();
    assert_eq!(answered_steers(&f), both);
    assert!(store.sent(f.id).unwrap().is_empty());
}
