//! Outcomes: what parses, and how the fold ends a run in exactly one.
#![allow(
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test fails loudly on a status it did not expect, and indexes what it just built"
)]

use super::*;
use crate::workflow::{StepReport, StepResult};

/// Where a Box with no outcomes stands once every gate passed: at the implicit `done`.
pub(super) fn passed() -> GateStatus {
    GateStatus::Passed {
        outcome: Outcome::implicit().verdict(),
    }
}

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

fn gate(name: &str, evidence: &str, checks: &[&str]) -> GateDoc {
    GateDoc {
        name: name.to_owned(),
        evidence: vec![evidence.to_owned()],
        checks: checks.iter().map(|c| CheckDoc::from(*c)).collect(),
        approval: None,
        max_tries: None,
    }
}

fn outcome(name: &str, checks: &[&str], anytime: bool, tone: Option<Tone>) -> OutcomeDoc {
    OutcomeDoc {
        name: name.to_owned(),
        says: None,
        tone,
        checks: checks.iter().map(|c| CheckDoc::from(*c)).collect(),
        anytime,
    }
}

fn gate_docs() -> Vec<GateDoc> {
    vec![
        gate("proof", "pr-*", &[]),
        gate("done", "posted.md", &["comment"]),
    ]
}

fn outcome_docs() -> Vec<OutcomeDoc> {
    vec![
        outcome("nothing to do", &["no-pr"], true, Some(Tone::Plain)),
        outcome("check failed", &["failed"], false, Some(Tone::Bad)),
        outcome("check passed", &["passed"], false, None),
    ]
}

/// The gated fixture: `proof`, then `done` with one check, and three outcomes.
fn fixture() -> Gates {
    Gates::parse(&gate_docs(), &outcome_docs()).unwrap()
}

fn submitted(g: &str) -> GateLine {
    GateLine::Submitted {
        gate: id(g),
        try_n: 1,
        report: StepReport {
            step: id(g),
            result: StepResult::Ok,
            evidence: vec!["x".to_owned()],
            note: String::new(),
        },
    }
}

fn held(stage: &str, try_n: u8, name: &str) -> GateLine {
    GateLine::Held {
        gate: id(stage),
        try_n,
        outcome: OutcomeName(name.to_owned()),
    }
}

fn none_held(try_n: u8) -> GateLine {
    GateLine::NoneHeld {
        gate: closing(),
        try_n,
        tried: vec![Tried {
            outcome: OutcomeName("check passed".to_owned()),
            check: 0,
            exit: 1,
            tail: "state is failure".to_owned(),
        }],
    }
}

/// Every gate handed in and its check passed.
fn through_gates() -> Vec<GateLine> {
    vec![
        submitted("proof"),
        submitted("done"),
        GateLine::Checked {
            gate: id("done"),
            try_n: 1,
            check: 0,
            exit: 0,
            tail: String::new(),
        },
    ]
}

fn deciding(try_n: u8) -> GateStatus {
    GateStatus::Open {
        gate: closing(),
        try_n,
        next: GateNext::Decide,
    }
}

fn won(name: &str) -> Verdict {
    match status(
        &fixture(),
        &[through_gates(), vec![held(CLOSING, 1, name)]].concat(),
    ) {
        GateStatus::Passed { outcome } => outcome,
        other => panic!("not passed: {other:?}"),
    }
}

fn names(due: &DueOutcomes<'_>) -> Vec<String> {
    due.outcomes.iter().map(|o| o.name.to_string()).collect()
}

#[test]
fn once_every_gate_passed_moor_decides_and_check_passed_holds() {
    let g = fixture();
    let lines = through_gates();
    assert_eq!(status(&g, &lines), deciding(1));
    assert!(status(&g, &lines).can_move());
    assert_eq!(due_check(&g, &lines), None);
    let due = due_outcomes(&g, &lines).unwrap();
    assert_eq!((due.stage.as_str(), due.try_n), (CLOSING, 1));
    assert_eq!(
        names(&due),
        ["nothing to do", "check failed", "check passed"]
    );
    let won = won("check passed");
    assert_eq!((won.name.as_str(), won.tone), ("check passed", Tone::Good));
    let ended = [lines, vec![held(CLOSING, 1, "check passed")]].concat();
    assert_eq!(passed_gates(&g, &ended), 2);
    assert!(!status(&g, &ended).can_move());
}

#[test]
fn a_failed_pull_request_check_is_a_bad_outcome_that_still_passes() {
    let won = won("check failed");
    assert_eq!((won.name.as_str(), won.tone), ("check failed", Tone::Bad));
}

#[test]
fn an_anytime_outcome_that_holds_while_the_first_gate_waits_ends_the_run() {
    let g = fixture();
    let due = due_outcomes(&g, &[]).unwrap();
    assert_eq!((due.stage.as_str(), due.try_n), ("proof", 1));
    assert_eq!(names(&due), ["nothing to do"]);
    let lines = [held("proof", 1, "nothing to do")];
    let GateStatus::Passed { outcome } = status(&g, &lines) else {
        panic!("{:?}", status(&g, &lines));
    };
    assert_eq!(
        (outcome.name.as_str(), outcome.tone),
        ("nothing to do", Tone::Plain)
    );
    assert_eq!(passed_gates(&g, &lines), 0, "the later gates are skipped");
    let reminded = [
        GateLine::Reminded {
            gate: id("proof"),
            try_n: 1,
        },
        held("proof", 1, "nothing to do"),
    ];
    assert!(matches!(status(&g, &reminded), GateStatus::Passed { .. }));
}

#[test]
fn only_an_anytime_outcome_ends_a_gate_and_only_while_the_gate_has_not_passed() {
    let g = fixture();
    let not_anytime = [held("proof", 1, "check passed")];
    assert!(matches!(
        status(&g, &not_anytime),
        GateStatus::Open {
            next: GateNext::Agent,
            ..
        }
    ));
    let stale = [submitted("proof"), held("proof", 1, "nothing to do")];
    assert_eq!(passed_gates(&g, &stale), 1);
    assert!(matches!(status(&g, &stale), GateStatus::Open { .. }));
    let running = [submitted("proof"), submitted("done")];
    let due = due_outcomes(&g, &running).unwrap();
    assert_eq!(
        (due.stage.as_str(), names(&due)),
        ("done", vec!["nothing to do".to_owned()]),
        "tried before the gate's checks"
    );
    let ended = [running.to_vec(), vec![held("done", 1, "nothing to do")]].concat();
    assert!(matches!(status(&g, &ended), GateStatus::Passed { .. }));
}

#[test]
fn no_outcome_holding_steers_again_then_is_stuck_until_a_person_reopens_it() {
    let g = fixture();
    let mut lines = through_gates();
    lines.push(none_held(1));
    assert_eq!(status(&g, &lines), deciding(2));
    lines.push(held(CLOSING, 1, "check passed"));
    assert_eq!(
        status(&g, &lines),
        deciding(2),
        "a stale try changes nothing"
    );
    lines.push(none_held(2));
    assert_eq!(
        status(&g, &lines),
        GateStatus::Stuck {
            gate: closing(),
            try_n: 2,
            why: "out of tries: no outcome held".to_owned(),
        }
    );
    assert!(status(&g, &lines).can_move());
    let w = wait(&g, &lines).unwrap();
    assert_eq!((w.gate.as_str(), w.kind), (CLOSING, WaitKind::Stuck));
    assert!(w.tail.unwrap().contains("state is failure"));
    lines.push(GateLine::Reopened {
        gate: closing(),
        by: "owner".to_owned(),
    });
    assert_eq!(status(&g, &lines), deciding(1));
    lines.push(held(CLOSING, 1, "check passed"));
    assert!(matches!(status(&g, &lines), GateStatus::Passed { .. }));
}

#[test]
fn with_no_outcomes_the_run_ends_in_done_as_soon_as_the_last_gate_passes() {
    let g = Gates::parse(&gate_docs(), &[]).unwrap();
    assert_eq!(
        due_outcomes(&g, &[]),
        None,
        "nothing to try before a reminder"
    );
    assert_eq!(
        status(&g, &through_gates()),
        GateStatus::Passed {
            outcome: Verdict {
                name: OutcomeName("done".to_owned()),
                tone: Tone::Good,
                says: None,
            }
        }
    );
    assert_eq!(passed_gates(&g, &through_gates()), 2);
    assert!(g.outcome_docs().is_empty());
    assert_eq!(g.doc().outcomes, Vec::new());
}

#[test]
fn an_otherwise_outcome_with_no_check_holds_without_a_line() {
    let docs = [
        outcome("nothing to do", &["no-pr"], true, None),
        outcome("done", &[], false, None),
    ];
    let g = Gates::parse(&gate_docs(), &docs).unwrap();
    let due = due_outcomes(&g, &through_gates()).unwrap();
    assert_eq!(names(&due), ["nothing to do", "done"]);
    let first = [outcome("done", &[], false, None)];
    let g = Gates::parse(&gate_docs(), &first).unwrap();
    assert!(matches!(
        status(&g, &through_gates()),
        GateStatus::Passed { .. }
    ));
}

#[test]
fn outcomes_read_back_as_written() {
    let g = fixture();
    let text = serde_json::to_string(&g.doc()).unwrap();
    assert_eq!(Gates::from_json(&text).unwrap(), g);
    assert_eq!(g.outcome_docs(), outcome_docs());
    let json = serde_json::to_value(&g.doc().outcomes[0]).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"name": "nothing to do", "tone": "plain", "checks": ["no-pr"], "anytime": true})
    );
}

fn refused(outcomes: &[OutcomeDoc]) -> GateError {
    Gates::parse(&gate_docs(), outcomes).unwrap_err()
}

#[test]
fn outcomes_that_could_skip_the_gates_or_never_settle_are_refused() {
    let got = refused(&[
        outcome("nothing to do", &[], true, None),
        outcome("done", &[], false, None),
    ]);
    assert_eq!(
        got,
        GateError::AnytimeWithoutCheck("nothing to do".to_owned())
    );
    let got = refused(&[
        outcome("otherwise", &[], false, None),
        outcome("check passed", &["passed"], false, None),
    ]);
    assert_eq!(got, GateError::OtherwiseNotLast("otherwise".to_owned()));
    let got = refused(&[
        outcome("a", &[], false, None),
        outcome("b", &[], false, None),
    ]);
    assert_eq!(got, GateError::OtherwiseNotLast("a".to_owned()));
    let got = refused(&[outcome("nothing to do", &["x"], true, None)]);
    assert_eq!(got, GateError::OnlyAnytime);
    let seven: Vec<OutcomeDoc> = (0..7)
        .map(|n| outcome(&format!("o{n}"), &["x"], false, None))
        .collect();
    assert_eq!(refused(&seven), GateError::TooManyOutcomes(7));
    assert!(Gates::parse(&gate_docs(), seven.get(..6).unwrap()).is_ok());
}

#[test]
fn an_outcome_name_is_one_short_unique_line() {
    for bad in ["", "  ", "one\ntwo", &"x".repeat(OUTCOME_NAME_MAX + 1)] {
        let got = refused(&[outcome(bad, &[], false, None)]);
        assert!(
            matches!(got, GateError::BadOutcomeName(_)),
            "{bad:?}: {got:?}"
        );
    }
    let got = refused(&[
        outcome("done", &["x"], false, None),
        outcome("done", &[], false, None),
    ]);
    assert_eq!(got, GateError::DuplicateOutcome("done".to_owned()));
    let long = OutcomeDoc {
        says: Some("x".repeat(SAYS_MAX + 1)),
        ..outcome("done", &[], false, None)
    };
    assert!(matches!(refused(&[long]), GateError::BadOutcome { .. }));
    let empty_check = outcome("done", &[" "], false, None);
    assert!(matches!(
        refused(&[empty_check]),
        GateError::BadOutcome { .. }
    ));
}

#[test]
fn no_gate_may_take_the_closing_stage_name() {
    let got = Gates::parse(&[gate(CLOSING, "x", &[])], &[]).unwrap_err();
    assert_eq!(got, GateError::ReservedName(CLOSING.to_owned()));
}
