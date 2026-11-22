//! A check moor got no answer from: once is tried again, twice in a row waits for a person.

use super::*;

fn busy(g: &str, try_n: u8, check: u8) -> GateLine {
    GateLine::NoAnswer {
        gate: id(g),
        try_n,
        check: CheckOf::Gate { check },
        why: Unanswered::NotRun {
            reason: "Box 3 is busy with another command".to_owned(),
        },
    }
}

fn outcome_busy(try_n: u8) -> GateLine {
    GateLine::NoAnswer {
        gate: closing(),
        try_n,
        check: CheckOf::Outcome {
            outcome: OutcomeName("merged".to_owned()),
            check: 0,
        },
        why: Unanswered::NotRun {
            reason: "Box 3 is busy with another command".to_owned(),
        },
    }
}

const WHY: &str = "moor, not the agent, got no answer from its checks twice in a row; the last \
                   was check 1 (`make`): Box 3 is busy with another command";

pub(super) fn unheard(g: &str, try_n: u8, why: &str, cause: NoAnswerCause) -> GateStatus {
    GateStatus::Stuck {
        gate: id(g),
        try_n,
        why: why.to_owned(),
        unanswered: Some(cause),
    }
}

pub(super) fn cause(check: CheckOf, command: &str, reason: Unanswered) -> NoAnswerCause {
    NoAnswerCause {
        check,
        command: Some(command.to_owned()),
        says: None,
        reason,
    }
}

fn make_busy() -> NoAnswerCause {
    let reason = "Box 3 is busy with another command".to_owned();
    cause(
        CheckOf::Gate { check: 0 },
        "make",
        Unanswered::NotRun { reason },
    )
}

fn make() -> Gates {
    gates(&[tries(2, doc("tests", &[], &["make", "lint"], None))])
}

#[test]
fn one_no_answer_leaves_the_check_due_again() {
    let g = make();
    let lines = [busy("tests", 1, 0)];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
    let due = due_check(&g, &lines).map(|d| (d.try_n, d.check));
    assert_eq!(due, Some((1, 0)));
}

#[test]
fn two_in_a_row_wait_for_a_person_and_spend_no_try() {
    let g = make();
    let lines = [busy("tests", 1, 0), busy("tests", 1, 0)];
    assert_eq!(status(&g, &lines), unheard("tests", 1, WHY, make_busy()));
    assert_eq!(due_check(&g, &lines), None, "a stuck gate runs nothing");
    assert_eq!(due_outcomes(&g, &lines), None);
}

#[test]
fn an_answer_in_between_starts_the_count_again() {
    let g = make();
    let lines = [
        busy("tests", 1, 0),
        checked("tests", 1, 0, 0),
        busy("tests", 1, 1),
    ];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
    let failed = [
        busy("tests", 1, 0),
        checked("tests", 1, 0, 1),
        busy("tests", 2, 0),
    ];
    assert_eq!(status(&g, &failed), open("tests", 2, GateNext::RunChecks));
}

#[test]
fn a_reopened_gate_counts_afresh() {
    let g = make();
    let lines = [
        busy("tests", 1, 0),
        busy("tests", 1, 0),
        reopened("tests"),
        busy("tests", 1, 0),
    ];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
    let again = [lines.as_slice(), &[busy("tests", 1, 0)]].concat();
    assert_eq!(status(&g, &again), unheard("tests", 1, WHY, make_busy()));
}

#[test]
fn a_line_of_another_try_is_no_answer_of_this_one() {
    let g = make();
    let lines = [busy("tests", 2, 0), busy("tests", 2, 0)];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
}

#[test]
fn at_the_closing_stage_two_in_a_row_wait_for_a_person_naming_the_outcome() {
    let merged = OutcomeDoc {
        name: "merged".to_owned(),
        says: None,
        tone: None,
        checks: vec![CheckDoc::from("gh pr view --json state")],
        anytime: false,
    };
    let g = Gates::parse(&[doc("tests", &[], &["make"], None)], &[merged]).unwrap();
    let through = checked("tests", 1, 0, 0);
    let once = [through.clone(), outcome_busy(1)];
    assert_eq!(status(&g, &once), open(CLOSING, 1, GateNext::Decide));
    let twice = [through, outcome_busy(1), outcome_busy(1)];
    let why = "moor, not the agent, got no answer from its checks twice in a row; the last was \
               outcome `merged` check 1 (`gh pr view --json state`): Box 3 is busy with another \
               command";
    let merged = CheckOf::Outcome {
        outcome: OutcomeName("merged".to_owned()),
        check: 0,
    };
    let reason = "Box 3 is busy with another command".to_owned();
    let busy = cause(
        merged,
        "gh pr view --json state",
        Unanswered::NotRun { reason },
    );
    assert_eq!(status(&g, &twice), unheard(CLOSING, 1, why, busy));
    assert_eq!(due_outcomes(&g, &twice), None);
}

#[test]
fn a_no_answer_stuck_carries_its_cause_as_data_and_an_ordinary_one_omits_it() {
    let g = make();
    let stuck = status(&g, &[busy("tests", 1, 0), busy("tests", 1, 0)]);
    let json = serde_json::to_value(&stuck).unwrap();
    let want = serde_json::json!({
        "state": "stuck", "gate": "tests", "try_n": 1, "why": WHY,
        "unanswered": {
            "check": {"gate": {"check": 0}},
            "command": "make",
            "reason": {"kind": "not_run", "reason": "Box 3 is busy with another command"},
        },
    });
    assert_eq!(json, want);
    let spent = status(&g, &[checked("tests", 1, 0, 1), checked("tests", 2, 0, 1)]);
    assert!(matches!(spent, GateStatus::Stuck { .. }), "{spent:?}");
    let json = serde_json::to_value(&spent).unwrap();
    assert!(json.get("unanswered").is_none(), "{json}");
}

#[test]
fn a_multi_line_command_stays_whole_in_the_cause_and_on_one_line_in_the_sentence() {
    let run = "make \\
  test";
    let g = gates(&[doc("tests", &[], &[run], None)]);
    let lines = [busy("tests", 1, 0), busy("tests", 1, 0)];
    let (why, unanswered) = match status(&g, &lines) {
        GateStatus::Stuck {
            why, unanswered, ..
        } => (why, unanswered),
        other => (format!("{other:?}"), None),
    };
    assert!(!why.contains('\n'), "{why}");
    assert!(why.contains("(`make \\ test`)"), "{why}");
    let cause = unanswered.unwrap();
    assert_eq!(cause.command.as_deref(), Some(run));
    let w = wait(&g, &lines).unwrap();
    assert_eq!(w.command.as_deref(), Some(run));
    assert_eq!(w.unanswered.as_ref(), Some(&cause));
    let line = crate::workflow::HistoryEntry::GateWaiting {
        step: id("tests"),
        try_n: 1,
        why: Some(why),
        unanswered: Some(cause),
    };
    let text = serde_json::to_string(&line).unwrap();
    let back: crate::workflow::HistoryEntry = serde_json::from_str(&text).unwrap();
    assert_eq!(back, line);
    let old = r#"{"kind": "gate_waiting", "step": "tests", "try_n": 1, "why": "out of tries"}"#;
    let old: crate::workflow::HistoryEntry = serde_json::from_str(old).unwrap();
    assert!(matches!(
        old,
        crate::workflow::HistoryEntry::GateWaiting {
            unanswered: None,
            ..
        }
    ));
}

#[test]
fn no_answers_from_two_checks_in_a_row_wait_naming_the_last_and_claim_no_more() {
    let anytime: OutcomeDoc = serde_json::from_value(serde_json::json!(
        {"name": "nothing to do", "checks": ["gh pr view"], "anytime": true}
    ))
    .unwrap();
    let done: OutcomeDoc = serde_json::from_value(serde_json::json!({"name": "done"})).unwrap();
    let g = Gates::parse(&[doc("tests", &[], &["make"], None)], &[anytime, done]).unwrap();
    let not_run = GateLine::NoAnswer {
        gate: id("tests"),
        try_n: 1,
        check: CheckOf::Outcome {
            outcome: OutcomeName("nothing to do".to_owned()),
            check: 0,
        },
        why: Unanswered::NotRun {
            reason: "Box 3 is busy with another command".to_owned(),
        },
    };
    // The anytime check then exits 1, which writes no line, and `make` loses its stream.
    let lines = [not_run, lost("tests", 1, 0)];
    let why = "moor, not the agent, got no answer from its checks twice in a row; the last was \
               check 1 (`make`): its output stream was lost";
    let gone = cause(CheckOf::Gate { check: 0 }, "make", Unanswered::Lost);
    assert_eq!(status(&g, &lines), unheard("tests", 1, why, gone));
}

#[test]
fn a_reopen_after_no_answers_keeps_the_report_so_the_check_is_due_at_once() {
    let g = gates(&[doc("tests", &["proof.md"], &["make"], None)]);
    let lines = [
        submitted("tests", 1),
        busy("tests", 1, 0),
        busy("tests", 1, 0),
        reopened("tests"),
    ];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::RunChecks));
    let due = due_check(&g, &lines).map(|d| (d.try_n, d.check));
    assert_eq!(due, Some((1, 0)));
}

#[test]
fn a_reopen_after_no_answers_at_a_later_try_keeps_that_try_and_its_report() {
    let g = gates(&[tries(
        3,
        doc("tests", &["proof.md"], &["make"], Some(Approval::Owner)),
    )]);
    let second = GateLine::Submitted {
        gate: id("tests"),
        try_n: 2,
        report: StepReport {
            step: id("tests"),
            result: StepResult::Ok,
            evidence: vec!["second.md".to_owned()],
            note: String::new(),
        },
    };
    let mut lines = vec![
        submitted("tests", 1),
        checked("tests", 1, 0, 1),
        second,
        busy("tests", 2, 0),
        busy("tests", 2, 0),
        reopened("tests"),
    ];
    assert_eq!(status(&g, &lines), open("tests", 2, GateNext::RunChecks));
    let due = due_check(&g, &lines).map(|d| (d.try_n, d.check));
    assert_eq!(due, Some((2, 0)));
    lines.push(checked("tests", 2, 0, 0));
    assert_eq!(status(&g, &lines), open("tests", 2, GateNext::Ask));
    assert_eq!(wait(&g, &lines).unwrap().handed, ["second.md"]);
}

#[test]
fn a_reopen_after_the_tries_ran_out_still_waits_for_the_agent() {
    let g = gates(&[tries(1, doc("tests", &["proof.md"], &["make"], None))]);
    let lines = [
        submitted("tests", 1),
        checked("tests", 1, 0, 1),
        reopened("tests"),
    ];
    assert_eq!(status(&g, &lines), open("tests", 1, GateNext::Agent));
}

#[test]
fn a_lost_stream_is_tried_again_and_two_wait_for_a_person() {
    let g = gates(&[tries(2, doc("tests", &[], &["a"], None))]);
    let once = [lost("tests", 1, 0)];
    assert_eq!(status(&g, &once), open("tests", 1, GateNext::RunChecks));
    let twice = [lost("tests", 1, 0), lost("tests", 1, 0)];
    let why = "moor, not the agent, got no answer from its checks twice in a row; the last was \
               check 1 (`a`): its output stream was lost";
    let gone = cause(CheckOf::Gate { check: 0 }, "a", Unanswered::Lost);
    assert_eq!(status(&g, &twice), unheard("tests", 1, why, gone));
    let then_ok = [lost("tests", 1, 0), checked("tests", 1, 0, 0)];
    assert_eq!(status(&g, &then_ok), passed());
}
