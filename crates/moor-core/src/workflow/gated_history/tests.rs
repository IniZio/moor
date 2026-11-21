//! What each gate line adds to the history of its run.

#![allow(clippy::panic, reason = "a test names the line it did not expect")]

use super::gate_history;
use crate::gate::{CheckDoc, GateDoc, GateLine, Gates};
use crate::workflow::{Answer, HistoryEntry, StepId, StepReport, StepResult};

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

fn gates() -> Gates {
    let evidence = |name: &str, files: &[&str], checks: &[&str]| GateDoc {
        name: name.to_owned(),
        evidence: files.iter().map(|s| (*s).to_owned()).collect(),
        checks: checks.iter().map(|s| CheckDoc::from(*s)).collect(),
        approval: None,
        max_tries: Some(2),
    };
    Gates::parse(&[
        evidence("proof", &["pr-*"], &[]),
        evidence("done", &["posted.md"], &["true"]),
    ])
    .unwrap()
}

fn submitted(gate: &str, try_n: u8) -> GateLine {
    GateLine::Submitted {
        gate: id(gate),
        try_n,
        report: StepReport {
            step: id(gate),
            result: StepResult::Ok,
            evidence: vec!["x".to_owned()],
            note: "n".to_owned(),
        },
    }
}

fn checked(gate: &str, try_n: u8, exit: i32) -> GateLine {
    GateLine::Checked {
        gate: id(gate),
        try_n,
        check: 0,
        exit,
        tail: String::new(),
    }
}

fn kinds(entries: &[HistoryEntry]) -> Vec<(&'static str, String)> {
    let name = |s: &StepId| s.to_string();
    entries
        .iter()
        .map(|e| match e {
            HistoryEntry::Reported { step, .. } => ("reported", name(step)),
            HistoryEntry::GateRefused { step, .. } => ("refused", name(step)),
            HistoryEntry::Checked { step, exit, .. } => ("checked", format!("{step} {exit}")),
            HistoryEntry::GatePassed { step } => ("passed", name(step)),
            HistoryEntry::Entered { step, try_n } => ("entered", format!("{step} {try_n}")),
            HistoryEntry::GateAnswered { step, .. } => ("answered", name(step)),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn an_accepted_report_that_passes_a_gate_puts_the_next_in_hand() {
    let got = gate_history(&gates(), &[], &submitted("proof", 1), false);
    let want = [
        ("reported", "proof"),
        ("passed", "proof"),
        ("entered", "done 1"),
    ];
    assert_eq!(kinds(&got), want.map(|(k, v)| (k, v.to_owned())));
}

#[test]
fn a_report_that_leaves_the_gate_waiting_for_its_check_adds_only_the_report() {
    let before = [submitted("proof", 1), submitted("done", 1)];
    let got = gate_history(&gates(), &before[..1], &before[1], false);
    assert_eq!(kinds(&got), [("reported", "done".to_owned())]);
}

#[test]
fn a_check_that_passes_the_last_gate_passes_it() {
    let before = [submitted("proof", 1), submitted("done", 1)];
    let got = gate_history(&gates(), &before, &checked("done", 1, 0), false);
    let want = [("checked", "done 0"), ("passed", "done")];
    assert_eq!(kinds(&got), want.map(|(k, v)| (k, v.to_owned())));
}

#[test]
fn a_failed_check_with_a_try_left_puts_the_gate_in_hand_again() {
    let before = [submitted("proof", 1), submitted("done", 1)];
    let got = gate_history(&gates(), &before, &checked("done", 1, 7), false);
    let want = [("checked", "done 7"), ("entered", "done 2")];
    assert_eq!(kinds(&got), want.map(|(k, v)| (k, v.to_owned())));
}

#[test]
fn a_run_that_waits_leaves_the_gate_in_hand_to_its_resume() {
    let before = [
        submitted("proof", 1),
        submitted("done", 1),
        checked("done", 1, 7),
    ];
    let line = checked("done", 2, 7);
    let got = gate_history(&gates(), &before, &line, true);
    assert_eq!(kinds(&got), [("checked", "done 7".to_owned())]);
    let answer = GateLine::Answered {
        gate: id("done"),
        try_n: 2,
        by: "me".to_owned(),
        answer: Answer::No,
        note: String::new(),
    };
    let got = gate_history(&gates(), &before, &answer, true);
    assert!(matches!(
        got.first(),
        Some(HistoryEntry::GateAnswered { note: None, .. })
    ));
}
