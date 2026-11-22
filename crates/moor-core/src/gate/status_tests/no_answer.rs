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

const WHY: &str =
    "moor, not the agent, could not run check 1 (`make`) twice: Box 3 is busy with another command";

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
    assert_eq!(status(&g, &lines), stuck("tests", 1, WHY));
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
    assert_eq!(status(&g, &again), stuck("tests", 1, WHY));
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
    let why = "moor, not the agent, could not run outcome `merged` check 1 \
               (`gh pr view --json state`) twice: Box 3 is busy with another command";
    assert_eq!(status(&g, &twice), stuck(CLOSING, 1, why));
    assert_eq!(due_outcomes(&g, &twice), None);
}
