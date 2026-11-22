//! How the run page words the lines a gated run writes.

#![allow(
    clippy::indexing_slicing,
    reason = "a test indexes the JSON it just built"
)]

use serde_json::json;

use super::{line_of, run_view};

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

fn row(
    history: serde_json::Value,
    verdict: &serde_json::Value,
) -> moor_core::view::workflow::RunRowView {
    serde_json::from_value(json!({
        "id": 3, "due_at": 1, "outcome": "finished", "phase": "done", "verdict": verdict,
        "history": history,
        "plan": {"steps": [
            {"id": "proof", "title": "proof", "kind": "gate"},
            {"id": "done", "title": "done", "kind": "gate"},
        ]},
    }))
    .unwrap()
}

fn states(view: &serde_json::Value) -> Vec<&str> {
    let steps = view["steps"].as_array().unwrap();
    steps.iter().map(|s| s["state"].as_str().unwrap()).collect()
}

const EARLY: &str = r#"[
    {"at": 1, "entry": {"kind": "entered", "step": "proof", "try_n": 1}},
    {"at": 2, "entry": {"kind": "outcome_reached", "outcome": {"name": "nothing to do", "tone": "plain"}}},
    {"at": 3, "entry": {"kind": "ended", "outcome": {"outcome": "delivered", "verdict": {"name": "nothing to do", "tone": "plain"}}}}
]"#;

#[test]
fn gates_an_anytime_outcome_ended_the_run_before_read_not_reached() {
    let verdict = json!({"name": "nothing to do", "tone": "plain"});
    let view = run_view(
        "evidence",
        &row(serde_json::from_str(EARLY).unwrap(), &verdict),
    );
    assert_eq!(states(&view), ["pending", "pending"]);
    assert_eq!(view["label"], "nothing to do");
    assert_eq!(view["status"], "success");
    assert_eq!(view["verdict"]["name"], "nothing to do");
    assert_eq!(view["closing"], serde_json::Value::Null);
    let lines = view["lines"].as_array().unwrap();
    assert_eq!(lines.last().unwrap()["text"], "Ended: nothing to do");
}

#[test]
fn a_run_that_passed_every_gate_keeps_them_passed_and_names_its_outcome() {
    let history = json!([
        {"at": 1, "entry": {"kind": "entered", "step": "proof", "try_n": 1}},
        {"at": 2, "entry": {"kind": "gate_passed", "step": "proof"}},
        {"at": 3, "entry": {"kind": "entered", "step": "done", "try_n": 1}},
        {"at": 4, "entry": {"kind": "gate_passed", "step": "done"}},
        {"at": 5, "entry": {"kind": "outcome_reached", "outcome": {"name": "check failed", "tone": "bad"}}},
    ]);
    let verdict = json!({"name": "check failed", "tone": "bad", "says": "The check failed."});
    let view = run_view("evidence", &row(history, &verdict));
    assert_eq!(states(&view), ["passed", "passed"]);
    assert_eq!(view["verdict"]["says"], "The check failed.");
}

#[test]
fn a_run_without_a_named_outcome_reads_as_it_did() {
    let history = json!([
        {"at": 1, "entry": {"kind": "entered", "step": "proof", "try_n": 1}},
        {"at": 2, "entry": {"kind": "gate_passed", "step": "proof"}},
        {"at": 3, "entry": {"kind": "ended", "outcome": {"outcome": "delivered", "result": "ok"}}},
    ]);
    let view = run_view("w", &row(history, &serde_json::Value::Null));
    assert_eq!(view["label"], "finished");
    assert_eq!(view["verdict"], serde_json::Value::Null);
    let lines = view["lines"].as_array().unwrap();
    assert_eq!(lines.last().unwrap()["text"], "Ended: delivered");
}
