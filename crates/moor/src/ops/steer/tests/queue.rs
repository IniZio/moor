//! A text steer the Box cannot take now waits in its inbox, and the inbox pass delivers it.

use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::inbox::InboxKind;
use moor_core::domain::inbox::QueueCause;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{EngineError, Steer};
use moor_core::view::steer::Delivery;

use super::{blocked, fx, fx_with, hold_lock, put_to_sleep, steered, text, Fx};
use crate::ops::steer::run;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

/// Runs the inbox pass once.
pub(super) fn pass(f: &Fx) {
    crate::ops::inbox_pass(&f.cx, f.cx.store().unwrap(), &|| false, &f.backoff).unwrap();
}

/// Makes the agent of `session` report Idle.
pub(super) fn reports_idle(f: &Fx, session: &str) {
    f.engine
        .set_probes(session, agent::idle(Ts::from_millis(i64::MAX / 2)));
}

/// Each text sent to an agent, oldest first.
fn texts(f: &Fx) -> Vec<String> {
    let sends = f.engine.steers().into_iter().map(|(_, s)| s);
    sends
        .filter_map(|s| match s {
            Steer::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// Queues `t` as a sender would, without the Box's turn.
fn queue(f: &Fx, t: &str) -> i64 {
    let store = f.cx.store().unwrap();
    store
        .enqueue(f.id, InboxKind::Steer, t, "steer")
        .unwrap()
        .get()
}

#[test]
fn a_steer_to_an_asleep_box_is_queued_and_the_pass_wakes_it_and_delivers_once() {
    let f = fx();
    put_to_sleep(&f);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(
        (v.delivery, v.steer),
        (Delivery::Queued(QueueCause::Asleep), None)
    );
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("wake ")));
    assert!(f.engine.steers().is_empty());
    reports_idle(&f, "agent-1");
    pass(&f);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert_eq!(texts(&f), ["hi"]);
    let sent = steered(&f);
    assert_eq!(sent.first().unwrap()["inbox"], serde_json::json!(v.inbox));
    pass(&f);
    assert_eq!(texts(&f), ["hi"], "sent once");
}

#[test]
fn a_steer_to_a_box_another_op_holds_is_queued_and_delivered_after() {
    let f = fx();
    let held = hold_lock(&f);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(v.delivery, Delivery::Queued(QueueCause::Busy));
    assert!(f.engine.steers().is_empty());
    reports_idle(&f, "agent-1");
    pass(&f);
    assert!(f.engine.steers().is_empty(), "the pass skips a held Box");
    drop(held);
    pass(&f);
    assert_eq!(texts(&f), ["hi"]);
}

#[test]
fn a_steer_to_a_box_that_lost_its_machine_is_queued_as_relaunching() {
    let f = fx();
    let gone = Decision::new(f.id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing);
    f.cx.store().unwrap().apply(gone).unwrap();
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(v.delivery, Delivery::Queued(QueueCause::Relaunching));
    assert!(f.engine.steers().is_empty());
}

#[test]
fn a_row_left_by_a_sender_that_went_away_is_delivered_by_the_pass() {
    let f = fx();
    let row = queue(&f, "late");
    reports_idle(&f, "agent-1");
    pass(&f);
    assert_eq!(texts(&f), ["late"]);
    assert_eq!(
        steered(&f).first().unwrap()["inbox"],
        serde_json::json!(row)
    );
}

#[test]
fn queued_rows_wait_while_the_agent_needs_input_then_go_oldest_first() {
    let f = fx_with(FakeEngine::new().probes("agent-1", blocked("Allow?")), true);
    queue(&f, "one");
    queue(&f, "two");
    pass(&f);
    assert!(f.engine.steers().is_empty());
    reports_idle(&f, "agent-1");
    pass(&f);
    assert_eq!(texts(&f), ["one", "two"]);
}

#[test]
fn a_steer_behind_queued_rows_is_delivered_after_them() {
    let f = fx();
    queue(&f, "first");
    reports_idle(&f, "agent-1");
    let v = run(&f.cx, &text("second")).unwrap();
    assert_eq!(v.delivery, Delivery::Delivered);
    assert!(v.steer.is_some());
    assert_eq!(texts(&f), ["first", "second"]);
}

#[test]
fn a_row_whose_two_sends_fail_is_reported_and_the_next_is_delivered() {
    let failed = || {
        Err(EngineError::Failed {
            step: "type",
            message: "pane busy".into(),
        })
    };
    let engine = FakeEngine::new()
        .steer_result(failed())
        .steer_result(failed());
    let f = fx_with(engine, true);
    let one = queue(&f, "one");
    queue(&f, "two");
    reports_idle(&f, "agent-1");
    for _ in 0..3 {
        pass(&f);
    }
    assert_eq!(texts(&f), ["one", "one", "two"]);
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    let unconfirmed: Vec<_> = events
        .into_iter()
        .filter(|e| e.kind == "steer_unconfirmed")
        .map(|e| e.payload)
        .collect();
    assert_eq!(
        unconfirmed,
        [serde_json::json!({"inbox": one, "text": "one"})]
    );
}

#[test]
fn a_row_in_doubt_on_an_agent_that_ended_is_reported_and_not_sent_again() {
    let f = fx();
    let store = f.cx.store().unwrap();
    let old = store.live_launch(f.id).unwrap().unwrap();
    let row = store
        .enqueue(f.id, InboxKind::Steer, "cut", "steer")
        .unwrap();
    store.mark_sending(f.id, row, old.id).unwrap();
    store.end_launch_with(old.id, None, None).unwrap();
    store.begin_launch(f.id, "agent-2").unwrap();
    queue(&f, "next");
    reports_idle(&f, "agent-2");
    pass(&f);
    assert_eq!(texts(&f), ["next"]);
    let events = store.events(f.id, 100).unwrap();
    let unconfirmed: Vec<_> = events
        .into_iter()
        .filter(|e| e.kind == "steer_unconfirmed")
        .map(|e| e.payload)
        .collect();
    assert_eq!(
        unconfirmed,
        [serde_json::json!({"inbox": row.get(), "text": "cut"})]
    );
}
