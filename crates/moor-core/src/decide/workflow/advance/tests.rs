//! Tests for where a quiet run goes and when a quiet reading counts.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use std::collections::HashMap;

use serde_json::json;

use super::{quiet_move, quiet_since, Move};
use crate::decide::workflow::{RunBoxFacts, QUIET_BEFORE_FINISHED};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;
use crate::workflow::{Plan, StepId};

mod check;
mod report;

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

fn work(id: &str, ok: &str, fail: Option<&str>, tries: u8) -> serde_json::Value {
    json!({
        "id": id, "title": id, "kind": "work", "max_tries": tries,
        "instructions": "do it", "done_when": "done", "evidence": ["e"],
        "ok": ok, "fail": fail
    })
}

fn plan(steps: &[serde_json::Value]) -> Plan {
    let deliver = json!({"id": "end", "title": "end", "kind": "deliver",
                         "instructions": "ship", "done_when": "shipped"});
    let mut all = steps.to_vec();
    all.push(deliver);
    Plan::parse(&json!({ "steps": all }).to_string(), "prompt").unwrap()
}

fn counts(entries: &[(&str, u8)]) -> impl Fn(&StepId) -> u8 {
    let map: HashMap<String, u8> = entries.iter().map(|(s, n)| ((*s).to_owned(), *n)).collect();
    move |s: &StepId| map.get(s.as_str()).copied().unwrap_or(0)
}

#[test]
fn a_work_step_passes_on_to_its_ok_edge() {
    let p = plan(&[work("a", "b", None, 1), work("b", "end", None, 1)]);
    let entered = counts(&[("a", 1)]);
    assert_eq!(
        quiet_move(&p, &id("a"), &entered),
        Move::Enter {
            step: id("b"),
            try_n: 1,
            exhausted: vec![]
        }
    );
}

#[test]
fn a_deliver_step_delivers() {
    let p = plan(&[work("a", "end", None, 1)]);
    assert_eq!(quiet_move(&p, &id("end"), &counts(&[])), Move::Delivered);
}

#[test]
fn a_loop_enters_a_step_again_until_its_tries_are_used() {
    let p = plan(&[work("a", "a", Some("end"), 3)]);
    let again = |n| quiet_move(&p, &id("a"), &counts(&[("a", n)]));
    assert_eq!(
        again(1),
        Move::Enter {
            step: id("a"),
            try_n: 2,
            exhausted: vec![]
        }
    );
    assert_eq!(
        again(2),
        Move::Enter {
            step: id("a"),
            try_n: 3,
            exhausted: vec![]
        }
    );
    assert_eq!(
        again(3),
        Move::Enter {
            step: id("end"),
            try_n: 1,
            exhausted: vec![id("a")]
        }
    );
}

#[test]
fn an_exhausted_step_with_no_fail_edge_fails_the_run() {
    let p = plan(&[work("a", "b", Some("end"), 3), work("b", "a", None, 1)]);
    let Move::Fail { at, exhausted, .. } = quiet_move(&p, &id("a"), &counts(&[("a", 2), ("b", 1)]))
    else {
        panic!("the run should fail");
    };
    assert_eq!(at, id("b"));
    assert_eq!(exhausted, vec![id("b")]);
}

#[test]
fn a_fail_target_that_is_exhausted_too_fails_the_run() {
    let p = plan(&[work("a", "a", Some("b"), 1), work("b", "end", None, 1)]);
    let entered = counts(&[("a", 1), ("b", 1)]);
    let Move::Fail { at, exhausted, .. } = quiet_move(&p, &id("a"), &entered) else {
        panic!("the run should fail");
    };
    assert_eq!(at, id("b"));
    assert_eq!(exhausted, vec![id("a"), id("b")]);
}

#[test]
fn every_run_ends_within_the_sum_of_the_tries() {
    let p = plan(&[work("a", "b", Some("end"), 2), work("b", "a", None, 2)]);
    let mut seen: HashMap<String, u8> = HashMap::new();
    seen.insert("a".to_owned(), 1);
    let mut at = id("a");
    let mut moves = 0;
    loop {
        let entered = |s: &StepId| seen.get(s.as_str()).copied().unwrap_or(0);
        match quiet_move(&p, &at, &entered) {
            Move::Enter { step, try_n, .. } => {
                seen.insert(step.to_string(), try_n);
                at = step;
                moves += 1;
            }
            Move::Delivered => break,
            other => panic!("the loop should end at the deliver step: {other:?}"),
        }
        assert!(moves <= 5, "the loop ran past the sum of its tries");
    }
    assert_eq!(at, id("end"));
    assert_eq!(moves, 4);
}

fn facts(session: SessionState, idle_since: Option<i64>) -> RunBoxFacts {
    RunBoxFacts {
        lifecycle: BoxLifecycle::Ready,
        session,
        idle_since: idle_since.map(Ts::from_millis),
        launch_failed: false,
    }
}

#[test]
fn an_idle_agent_counts_only_when_it_went_idle_after_the_send() {
    let sent = Ts::from_millis(1_000_000);
    let now = Ts::from_millis(9_000_000);
    assert!(!quiet_since(
        &facts(SessionState::Idle, Some(999_999)),
        sent,
        now
    ));
    assert!(quiet_since(
        &facts(SessionState::Idle, Some(1_000_000)),
        sent,
        now
    ));
    assert!(!quiet_since(&facts(SessionState::Idle, None), sent, now));
}

#[test]
fn a_working_agent_never_counts() {
    let t = Ts::from_millis(0);
    for s in [SessionState::Working, SessionState::Starting] {
        assert!(!quiet_since(
            &facts(s, None),
            t,
            Ts::from_millis(i64::MAX / 2)
        ));
    }
}

#[test]
fn an_ended_agent_counts_after_a_quiet_wait_from_the_send() {
    let sent = Ts::from_millis(1_000_000);
    let wait = i64::try_from(QUIET_BEFORE_FINISHED.as_millis()).unwrap();
    let ended = facts(SessionState::Ended, None);
    assert!(!quiet_since(
        &ended,
        sent,
        Ts::from_millis(1_000_000 + wait - 1)
    ));
    assert!(quiet_since(&ended, sent, Ts::from_millis(1_000_000 + wait)));
}
