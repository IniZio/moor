//! Tests for check steps, lost streams and ended agents, through `advance`.
#![allow(clippy::panic, reason = "test assertions panic on a wrong shape")]

use serde_json::json;

use super::{id, plan, work};
use crate::decide::workflow::{advance, Event, Move};
use crate::workflow::{HistoryEntry, Plan, StepId};

fn check(id: &str, ok: &str, fail: Option<&str>, tries: u8) -> serde_json::Value {
    json!({
        "id": id, "title": id, "kind": "check", "max_tries": tries,
        "command": "make test", "ok": ok, "fail": fail
    })
}

fn entered(step: &str, try_n: u8) -> HistoryEntry {
    HistoryEntry::Entered {
        step: id(step),
        try_n,
    }
}

fn checked(step: &str, try_n: u8, exit: i32) -> HistoryEntry {
    HistoryEntry::Checked {
        step: id(step),
        try_n,
        exit,
        tail: String::new(),
    }
}

fn lost(step: &str, try_n: u8) -> HistoryEntry {
    HistoryEntry::CheckLost {
        step: id(step),
        try_n,
    }
}

fn reminded(step: &str, try_n: u8) -> HistoryEntry {
    HistoryEntry::Reminded {
        step: id(step),
        try_n,
    }
}

fn enter(step: &str, try_n: u8, exhausted: &[&str]) -> Move {
    Move::Enter {
        step: id(step),
        try_n,
        exhausted: exhausted.iter().map(|s| id(s)).collect::<Vec<StepId>>(),
    }
}

/// `fix` (work) then `test` (check): ok ships, fail loops back to `fix`.
fn fix_and_test(fix_tries: u8, test_tries: u8) -> Plan {
    plan(&[
        work("fix", "test", None, fix_tries),
        check("test", "end", Some("fix"), test_tries),
    ])
}

#[test]
fn a_check_that_has_not_run_runs() {
    let p = fix_and_test(2, 3);
    let h = [entered("fix", 1), entered("test", 1)];
    assert_eq!(advance(&p, &h, Event::Idle), Move::RunCheck);
    assert_eq!(advance(&p, &h, Event::Ended), Move::RunCheck);
}

#[test]
fn a_check_that_exits_zero_follows_ok() {
    let p = fix_and_test(2, 3);
    let h = [entered("fix", 1), entered("test", 1), checked("test", 1, 0)];
    assert_eq!(advance(&p, &h, Event::Idle), enter("end", 1, &[]));
}

#[test]
fn a_check_that_exits_non_zero_follows_fail() {
    let p = fix_and_test(2, 3);
    let h = [entered("fix", 1), entered("test", 1), checked("test", 1, 1)];
    assert_eq!(advance(&p, &h, Event::Idle), enter("fix", 2, &[]));
}

#[test]
fn a_failed_check_with_no_fail_edge_fails_the_run() {
    let p = plan(&[check("test", "end", None, 1)]);
    let h = [entered("test", 1), checked("test", 1, 2)];
    let Move::Fail { at, reason, .. } = advance(&p, &h, Event::Idle) else {
        panic!("the run should fail");
    };
    assert_eq!(at, id("test"));
    assert!(reason.contains("exited 2"), "{reason}");
}

#[test]
fn a_fix_and_check_loop_exhausts_and_takes_the_fix_steps_fail_edge() {
    // fix may be entered twice and its own fail edge leads to `end`.
    let p = plan(&[
        work("fix", "test", Some("end"), 2),
        check("test", "end", Some("fix"), 3),
    ]);
    let mut h = vec![entered("fix", 1), entered("test", 1), checked("test", 1, 1)];
    assert_eq!(advance(&p, &h, Event::Idle), enter("fix", 2, &[]));
    h.extend([entered("fix", 2), entered("test", 2), checked("test", 2, 1)]);
    // fix is out of tries, so the run takes fix's fail edge instead.
    assert_eq!(advance(&p, &h, Event::Idle), enter("end", 1, &["fix"]));
}

#[test]
fn double_exhaustion_ends_the_run_failed() {
    // test fails into fix; fix is out of tries; fix's fail target, test, is out too.
    let p = plan(&[
        work("fix", "test", Some("test"), 1),
        check("test", "end", Some("fix"), 1),
    ]);
    let h = [entered("fix", 1), entered("test", 1), checked("test", 1, 1)];
    let Move::Fail { at, exhausted, .. } = advance(&p, &h, Event::Idle) else {
        panic!("the run should fail");
    };
    assert_eq!(at, id("test"));
    assert_eq!(exhausted, vec![id("fix"), id("test")]);
}

#[test]
fn a_lost_stream_is_no_result_and_the_check_runs_again() {
    let p = fix_and_test(2, 3);
    let h = [entered("fix", 1), entered("test", 1), lost("test", 1)];
    assert_eq!(advance(&p, &h, Event::Idle), Move::RunCheck);
}

#[test]
fn the_second_lost_stream_in_one_try_follows_fail() {
    let p = fix_and_test(2, 3);
    let h = [
        entered("fix", 1),
        entered("test", 1),
        lost("test", 1),
        lost("test", 1),
    ];
    assert_eq!(advance(&p, &h, Event::Idle), enter("fix", 2, &[]));
}

#[test]
fn lost_streams_of_an_earlier_try_do_not_count() {
    let p = fix_and_test(3, 3);
    let h = [
        entered("fix", 1),
        entered("test", 1),
        lost("test", 1),
        lost("test", 1),
        entered("fix", 2),
        entered("test", 2),
        lost("test", 2),
    ];
    assert_eq!(advance(&p, &h, Event::Idle), Move::RunCheck);
}

#[test]
fn a_lost_stream_then_a_result_uses_the_result() {
    let p = fix_and_test(2, 3);
    let h = [
        entered("fix", 1),
        entered("test", 1),
        lost("test", 1),
        checked("test", 1, 0),
    ];
    assert_eq!(advance(&p, &h, Event::Idle), enter("end", 1, &[]));
}

#[test]
fn an_ended_agent_on_a_work_step_is_sent_the_step_again_once() {
    let p = fix_and_test(2, 3);
    let first = [entered("fix", 1)];
    assert_eq!(advance(&p, &first, Event::Ended), Move::Remind);
    let after = [entered("fix", 1), reminded("fix", 1)];
    assert_eq!(
        advance(&p, &after, Event::Ended),
        Move::Incomplete { at: id("fix") }
    );
}

#[test]
fn an_idle_agent_with_no_report_is_reminded_once_and_then_incomplete() {
    let p = fix_and_test(2, 3);
    assert_eq!(advance(&p, &[entered("fix", 1)], Event::Idle), Move::Remind);
    let h = [entered("fix", 1), reminded("fix", 1)];
    assert_eq!(
        advance(&p, &h, Event::Idle),
        Move::Incomplete { at: id("fix") }
    );
}

#[test]
fn a_reminder_belongs_to_its_try() {
    let p = fix_and_test(2, 3);
    let h = [
        entered("fix", 1),
        reminded("fix", 1),
        entered("test", 1),
        checked("test", 1, 1),
        entered("fix", 2),
    ];
    assert_eq!(advance(&p, &h, Event::Ended), Move::Remind);
}

#[test]
fn a_planned_deliver_step_waits_for_its_report_like_a_work_step() {
    let p = plan(&[]);
    let h = [entered("end", 1)];
    assert_eq!(advance(&p, &h, Event::Ended), Move::Remind);
    assert_eq!(advance(&p, &h, Event::Idle), Move::Remind);
    let after = [entered("end", 1), reminded("end", 1)];
    assert_eq!(
        advance(&p, &after, Event::Idle),
        Move::Incomplete { at: id("end") }
    );
}

#[test]
fn the_deliver_step_of_a_workflow_nobody_planned_delivers_when_quiet() {
    let p = Plan::single("fix what is broken");
    let h = [entered("deliver", 1)];
    assert_eq!(advance(&p, &h, Event::Ended), Move::Delivered);
    assert_eq!(advance(&p, &h, Event::Idle), Move::Delivered);
}
