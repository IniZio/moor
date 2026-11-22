//! The state machine of a Box's gates, driven by the lines written for it.

use super::outcome_tests::passed;
use super::*;
use crate::workflow::{Answer, StepReport, StepResult};

fn doc(name: &str, evidence: &[&str], checks: &[&str], approval: Option<Approval>) -> GateDoc {
    GateDoc {
        name: name.to_owned(),
        evidence: evidence.iter().map(|s| (*s).to_owned()).collect(),
        checks: checks.iter().map(|s| CheckDoc::from(*s)).collect(),
        approval,
        max_tries: None,
    }
}

fn tries(n: u64, mut d: GateDoc) -> GateDoc {
    d.max_tries = Some(n);
    d
}

fn gates(docs: &[GateDoc]) -> Gates {
    Gates::parse(docs, &[]).unwrap()
}

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

fn open(g: &str, try_n: u8, next: GateNext) -> GateStatus {
    GateStatus::Open {
        gate: id(g),
        try_n,
        next,
    }
}

fn stuck(g: &str, try_n: u8, why: &str) -> GateStatus {
    GateStatus::Stuck {
        gate: id(g),
        try_n,
        why: why.to_owned(),
        unanswered: None,
    }
}

fn report(g: &str, result: StepResult) -> StepReport {
    StepReport {
        step: id(g),
        result,
        evidence: vec!["proof.md".to_owned()],
        note: String::new(),
    }
}

fn submitted(g: &str, try_n: u8) -> GateLine {
    GateLine::Submitted {
        gate: id(g),
        try_n,
        report: report(g, StepResult::Ok),
    }
}

fn refused(g: &str, try_n: u8) -> GateLine {
    GateLine::Refused {
        gate: id(g),
        try_n,
        missing: vec!["proof.md".to_owned()],
    }
}

fn checked(g: &str, try_n: u8, check: u8, exit: i32) -> GateLine {
    GateLine::Checked {
        gate: id(g),
        try_n,
        check,
        exit,
        tail: String::new(),
    }
}

fn lost(g: &str, try_n: u8, check: u8) -> GateLine {
    GateLine::NoAnswer {
        gate: id(g),
        try_n,
        check: CheckOf::Gate { check },
        why: Unanswered::Lost,
    }
}

fn reminded(g: &str, try_n: u8) -> GateLine {
    GateLine::Reminded { gate: id(g), try_n }
}

fn quiet(g: &str, try_n: u8) -> GateLine {
    GateLine::Quiet { gate: id(g), try_n }
}

fn answered(g: &str, try_n: u8, answer: Answer) -> GateLine {
    GateLine::Answered {
        gate: id(g),
        try_n,
        by: "owner".to_owned(),
        answer,
        note: String::new(),
    }
}

fn failed_try(g: &str, try_n: u8) -> GateLine {
    GateLine::TryFailed {
        gate: id(g),
        try_n,
        why: "because".to_owned(),
    }
}

fn reopened(g: &str) -> GateLine {
    GateLine::Reopened {
        gate: id(g),
        by: "owner".to_owned(),
    }
}

fn evidence_gate() -> Gates {
    gates(&[doc("proof", &["proof.md"], &[], None)])
}

#[test]
fn a_gate_that_needs_evidence_opens_on_the_agent() {
    assert_eq!(
        status(&evidence_gate(), &[]),
        open("proof", 1, GateNext::Agent)
    );
}

#[test]
fn a_gate_with_no_evidence_opens_on_its_checks_or_its_question() {
    let checks = gates(&[doc("tests", &[], &["true"], None)]);
    assert_eq!(status(&checks, &[]), open("tests", 1, GateNext::RunChecks));
    let ask = gates(&[doc("ok", &[], &[], Some(Approval::Owner))]);
    assert_eq!(status(&ask, &[]), open("ok", 1, GateNext::Ask));
}

#[test]
fn an_accepted_report_passes_a_gate_that_asks_nothing_more() {
    let lines = [submitted("proof", 1)];
    assert_eq!(status(&evidence_gate(), &lines), passed());
}

#[test]
fn an_accepted_report_leads_to_the_checks_then_the_question_then_passes() {
    let g = gates(&[doc(
        "all",
        &["proof.md"],
        &["a", "b"],
        Some(Approval::Owner),
    )]);
    let mut lines = vec![submitted("all", 1)];
    assert_eq!(status(&g, &lines), open("all", 1, GateNext::RunChecks));
    lines.push(checked("all", 1, 0, 0));
    assert_eq!(status(&g, &lines), open("all", 1, GateNext::RunChecks));
    lines.push(checked("all", 1, 1, 0));
    assert_eq!(status(&g, &lines), open("all", 1, GateNext::Ask));
    lines.push(answered("all", 1, Answer::Yes));
    assert_eq!(status(&g, &lines), passed());
}

#[test]
fn a_report_that_says_fail_fails_the_try() {
    let g = gates(&[tries(2, doc("proof", &["proof.md"], &[], None))]);
    let line = GateLine::Submitted {
        gate: id("proof"),
        try_n: 1,
        report: report("proof", StepResult::Fail),
    };
    assert_eq!(status(&g, &[line]), open("proof", 2, GateNext::Agent));
}

#[test]
fn one_refusal_keeps_the_try_and_a_second_fails_it() {
    let g = gates(&[tries(2, doc("proof", &["proof.md"], &[], None))]);
    let mut lines = vec![refused("proof", 1)];
    assert_eq!(status(&g, &lines), open("proof", 1, GateNext::Agent));
    lines.push(refused("proof", 1));
    assert_eq!(status(&g, &lines), open("proof", 2, GateNext::Agent));
}

#[test]
fn a_refusal_then_an_accepted_report_passes() {
    let lines = [refused("proof", 1), submitted("proof", 1)];
    assert_eq!(status(&evidence_gate(), &lines), passed());
}

#[test]
fn a_written_try_failed_fails_the_try_once() {
    let g = gates(&[tries(3, doc("proof", &["proof.md"], &[], None))]);
    let once = [failed_try("proof", 1)];
    assert_eq!(status(&g, &once), open("proof", 2, GateNext::Agent));
    let twice = [failed_try("proof", 1), failed_try("proof", 1)];
    assert_eq!(status(&g, &twice), open("proof", 2, GateNext::Agent));
}

#[test]
fn a_quiet_agent_is_reminded_once_then_stuck() {
    let mut lines = vec![reminded("proof", 1)];
    assert_eq!(
        status(&evidence_gate(), &lines),
        open("proof", 1, GateNext::Agent)
    );
    lines.push(quiet("proof", 1));
    assert_eq!(
        status(&evidence_gate(), &lines),
        stuck("proof", 1, "the agent went quiet")
    );
}

#[test]
fn quiet_without_a_reminder_first_changes_nothing() {
    assert_eq!(
        status(&evidence_gate(), &[quiet("proof", 1)]),
        open("proof", 1, GateNext::Agent)
    );
}

#[test]
fn a_reminded_agent_that_then_reports_passes() {
    let lines = [reminded("proof", 1), submitted("proof", 1)];
    assert_eq!(status(&evidence_gate(), &lines), passed());
}

#[test]
fn the_next_try_is_reminded_again() {
    let g = gates(&[tries(2, doc("proof", &["proof.md"], &[], None))]);
    let lines = [
        reminded("proof", 1),
        failed_try("proof", 1),
        reminded("proof", 2),
        quiet("proof", 2),
    ];
    assert_eq!(
        status(&g, &lines),
        stuck("proof", 2, "the agent went quiet")
    );
}

#[test]
fn checks_stop_at_the_first_failure() {
    let g = gates(&[tries(2, doc("tests", &[], &["a", "b"], None))]);
    let lines = [checked("tests", 1, 0, 1)];
    assert_eq!(status(&g, &lines), open("tests", 2, GateNext::RunChecks));
    let after = [checked("tests", 1, 0, 1), checked("tests", 1, 1, 0)];
    assert_eq!(status(&g, &after), open("tests", 2, GateNext::RunChecks));
}

#[test]
fn quiet_and_reminders_mean_nothing_while_the_checks_run() {
    let g = gates(&[doc("tests", &[], &["a"], None)]);
    let lines = [reminded("tests", 1), quiet("tests", 1)];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
}

#[test]
fn an_owner_no_fails_the_try_and_a_yes_passes() {
    let g = gates(&[tries(2, doc("ok", &[], &[], Some(Approval::Owner)))]);
    let no = [answered("ok", 1, Answer::No)];
    assert_eq!(status(&g, &no), open("ok", 2, GateNext::Ask));
    let yes = [
        answered("ok", 1, Answer::No),
        answered("ok", 2, Answer::Yes),
    ];
    assert_eq!(status(&g, &yes), passed());
}

#[test]
fn an_owner_no_on_the_last_try_fails_the_gate() {
    let g = gates(&[tries(1, doc("ok", &[], &[], Some(Approval::Owner)))]);
    let got = status(&g, &[answered("ok", 1, Answer::No)]);
    assert!(
        matches!(&got, GateStatus::Failed { gate, .. } if *gate == id("ok")),
        "{got:?}"
    );
}

#[test]
fn a_failed_gate_stays_failed() {
    let g = gates(&[tries(1, doc("ok", &[], &[], Some(Approval::Owner)))]);
    let lines = [
        answered("ok", 1, Answer::No),
        reopened("ok"),
        answered("ok", 1, Answer::Yes),
    ];
    assert!(matches!(status(&g, &lines), GateStatus::Failed { .. }));
}

#[test]
fn using_up_the_tries_leaves_the_gate_stuck() {
    let g = gates(&[tries(2, doc("proof", &["proof.md"], &[], None))]);
    let out = stuck("proof", 2, "out of tries: because");
    let lines = [failed_try("proof", 1), failed_try("proof", 2)];
    assert_eq!(status(&g, &lines), out);
    let more = [
        failed_try("proof", 1),
        failed_try("proof", 2),
        submitted("proof", 3),
    ];
    assert_eq!(status(&g, &more), out);
}

#[test]
fn a_reopened_gate_gets_fresh_tries_counted_from_the_reopening() {
    let g = gates(&[tries(2, doc("proof", &["proof.md"], &[], None))]);
    let mut lines = vec![
        failed_try("proof", 1),
        failed_try("proof", 2),
        reopened("proof"),
    ];
    assert_eq!(status(&g, &lines), open("proof", 1, GateNext::Agent));
    lines.push(failed_try("proof", 1));
    assert_eq!(status(&g, &lines), open("proof", 2, GateNext::Agent));
    lines.push(failed_try("proof", 2));
    assert_eq!(
        status(&g, &lines),
        stuck("proof", 2, "out of tries: because")
    );
}

#[test]
fn a_quiet_gate_reopens_too() {
    let lines = [reminded("proof", 1), quiet("proof", 1), reopened("proof")];
    assert_eq!(
        status(&evidence_gate(), &lines),
        open("proof", 1, GateNext::Agent)
    );
}

#[test]
fn a_gate_that_is_not_stuck_ignores_a_reopening() {
    let g = gates(&[tries(2, doc("proof", &["proof.md"], &[], None))]);
    let lines = [failed_try("proof", 1), reopened("proof")];
    assert_eq!(status(&g, &lines), open("proof", 2, GateNext::Agent));
}

#[test]
fn gates_pass_one_after_another_and_the_last_one_passes_the_box() {
    let g = gates(&[
        doc("proof", &["proof.md"], &[], None),
        doc("tests", &[], &["a"], None),
    ]);
    let mut lines = vec![submitted("proof", 1)];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
    lines.push(checked("tests", 1, 0, 0));
    assert_eq!(status(&g, &lines), passed());
}

#[test]
fn lines_of_a_stale_try_or_another_gate_change_nothing() {
    let g = gates(&[
        tries(2, doc("proof", &["proof.md"], &[], None)),
        doc("tests", &[], &["a"], None),
    ]);
    let lines = [
        failed_try("proof", 1),
        submitted("proof", 1),
        checked("tests", 1, 0, 0),
        answered("proof", 1, Answer::Yes),
    ];
    assert_eq!(status(&g, &lines), open("proof", 2, GateNext::Agent));
}

#[test]
fn a_passed_gate_ignores_its_late_lines() {
    let g = gates(&[
        doc("proof", &["proof.md"], &[], None),
        doc("tests", &[], &["a"], None),
    ]);
    let lines = [submitted("proof", 1), failed_try("proof", 1)];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
}

#[test]
fn a_check_out_of_order_changes_nothing() {
    let g = gates(&[doc("tests", &[], &["a", "b"], None)]);
    let lines = [checked("tests", 1, 1, 0)];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
}

mod answered;
mod due;
mod no_answer;
