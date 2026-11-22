//! How the run page words the lines a gated run writes.

#![allow(
    clippy::indexing_slicing,
    reason = "a test indexes the JSON it just built"
)]

use serde_json::json;

use super::line_of;

fn said(entry: &serde_json::Value) -> String {
    line_of(0, entry, None)["text"].as_str().unwrap().to_owned()
}

#[test]
fn a_check_line_leads_with_what_the_check_says_else_its_command() {
    assert_eq!(
        said(
            &json!({"kind": "checked", "step": "posted", "try_n": 1, "exit": 1, "check": "The pull request has one evidence comment."})
        ),
        "The pull request has one evidence comment: exited 1 (try 1)"
    );
    assert_eq!(
        said(
            &json!({"kind": "checked", "step": "posted", "try_n": 2, "exit": 0, "check": "`make test`"})
        ),
        "`make test`: exited 0 (try 2)"
    );
    assert_eq!(
        said(&json!({"kind": "checked", "step": "build", "try_n": 1, "exit": 1})),
        "Check exited 1 (try 1)"
    );
}

#[test]
fn a_gated_runs_lines_read_as_what_happened() {
    assert_eq!(
        said(&json!({"kind": "gate_passed", "step": "proof"})),
        "Gate passed"
    );
    assert_eq!(
        said(&json!({"kind": "gate_waiting", "step": "done", "try_n": 2, "why": "out of tries"})),
        "Waiting for a person (try 2): out of tries"
    );
    assert_eq!(
        said(&json!({"kind": "gate_waiting", "step": "ship", "try_n": 1})),
        "Waiting for the owner's answer (try 1)"
    );
    assert_eq!(
        said(
            &json!({"kind": "gate_answered", "step": "ship", "try_n": 1, "answer": "no", "note": "fix it"})
        ),
        "Owner answered no (try 1): fix it"
    );
    assert_eq!(
        said(&json!({"kind": "resumed", "step": "ship"})),
        "Resumed after the wait"
    );
}

#[test]
fn a_wait_for_want_of_answers_passes_its_cause_to_the_page_and_others_have_none() {
    let cause =
        json!({"check": {"gate": {"check": 0}}, "command": "make", "reason": {"kind": "lost"}});
    let entry = json!({"kind": "gate_waiting", "step": "done", "try_n": 1, "why": "w", "unanswered": cause});
    assert_eq!(line_of(0, &entry, None)["unanswered"], cause);
    let plain = json!({"kind": "gate_waiting", "step": "done", "try_n": 2, "why": "out of tries"});
    assert!(line_of(0, &plain, None).get("unanswered").is_none());
}
