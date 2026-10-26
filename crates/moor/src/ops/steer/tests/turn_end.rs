//! A steer's turn is answered by the first idle report of that turn, even one the agent wrote
//! while the steer was still being sent, and never by the end of a turn already running.

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneRead, Probes};
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;

use super::{fx_with, text, working, Fx};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::steer::run;
use crate::store::EventRow;
use crate::testing::engine::FakeEngine;

/// A report written at `at` in the turn whose prompt was submitted at `turn`.
fn report(kind: HookKind, at: i64, turn: i64) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(at),
            turn: Some(Ts::from_millis(turn)),
        }),
        pane: PaneRead::Absent,
    }
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
        .probes("agent-1", report(HookKind::Idle, 0, 0))
        .probes_after_steer("agent-1", report(HookKind::Idle, 1, 1));
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
        .probes("agent-1", report(HookKind::Busy, 10, 5))
        .probes_after_steer("agent-1", report(HookKind::Idle, 20, 5));
    let f = fx_with(engine, true);
    run(&f.cx, &text("next")).unwrap();
    settle_once(&f);
    assert!(
        turns_done(&f).is_empty(),
        "the earlier turn's stop is not this steer's"
    );
    f.engine
        .set_probes("agent-1", report(HookKind::Idle, 40, 30));
    settle_once(&f);
    assert_eq!(
        turns_done(&f).len(),
        1,
        "the steer's own turn end answers it"
    );
}

#[test]
fn a_steer_whose_agent_was_never_read_is_refused_unsent() {
    let unread = Probes {
        hook: HookRead::TimedOut,
        pane: PaneRead::TimedOut,
    };
    let f = fx_with(FakeEngine::new().probes("agent-1", unread), true);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert!(
        err.error.to_string().contains("could not read the agent"),
        "with no report to place the turn after, a stale idle report could answer it: {err:?}"
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
