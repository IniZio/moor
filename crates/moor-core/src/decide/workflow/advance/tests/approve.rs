//! Tests for the move an approve step and its answer make.

use serde_json::json;

use super::{id, plan, work};
use crate::decide::workflow::{advance, answer_move, Event, Move};
use crate::user::UserId;
use crate::workflow::{Answer, HistoryEntry, Plan};

fn approve(id: &str, ok: &str, fail: Option<&str>) -> serde_json::Value {
    json!({"id": id, "title": id, "kind": "approve", "question": "ship it?",
           "ok": ok, "fail": fail, "max_tries": 2})
}

fn entered(step: &str, try_n: u8) -> HistoryEntry {
    HistoryEntry::Entered {
        step: id(step),
        try_n,
    }
}

fn enter(step: &str) -> Move {
    Move::Enter {
        step: id(step),
        try_n: 1,
        exhausted: vec![],
    }
}

fn with_edge() -> Plan {
    plan(&[
        work("a", "ask", None, 1),
        approve("ask", "end", Some("redo")),
        work("redo", "end", None, 1),
    ])
}

fn no_edge() -> Plan {
    plan(&[work("a", "ask", None, 1), approve("ask", "end", None)])
}

#[test]
fn a_run_at_an_approve_step_asks() {
    let h = [entered("a", 1), entered("ask", 1)];
    assert_eq!(advance(&with_edge(), &h, Event::Idle), Move::Ask);
    assert_eq!(advance(&with_edge(), &h, Event::Ended), Move::Ask);
}

#[test]
fn yes_follows_the_ok_edge() {
    let h = [entered("a", 1), entered("ask", 1)];
    assert_eq!(answer_move(&with_edge(), &h, Answer::Yes), enter("end"));
}

#[test]
fn no_follows_the_fail_edge() {
    let h = [entered("a", 1), entered("ask", 1)];
    assert_eq!(answer_move(&with_edge(), &h, Answer::No), enter("redo"));
}

#[test]
fn no_with_no_fail_edge_declines_the_run() {
    let h = [entered("a", 1), entered("ask", 1)];
    assert_eq!(
        answer_move(&no_edge(), &h, Answer::No),
        Move::Declined { at: id("ask") }
    );
    assert_eq!(answer_move(&no_edge(), &h, Answer::Yes), enter("end"));
}

#[test]
fn an_answer_with_nothing_entered_moves_nothing() {
    assert_eq!(answer_move(&no_edge(), &[], Answer::Yes), Move::Wait);
}

#[test]
fn an_answer_line_is_not_a_pointer() {
    let h = [
        entered("a", 1),
        entered("ask", 1),
        HistoryEntry::Answered {
            step: id("ask"),
            by: UserId::new(1),
            answer: Answer::Yes,
            note: None,
        },
    ];
    assert_eq!(answer_move(&no_edge(), &h, Answer::Yes), enter("end"));
}
