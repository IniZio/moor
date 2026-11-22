//! How a run's outcome reaches the list rows and the run page.

use moor_core::view::workflow::LastRunView;
use serde_json::{json, Value};

use super::{closing, label_of, named};

fn run(extra: &Value) -> LastRunView {
    let mut base = json!({"due_at": 1, "outcome": "finished", "phase": "done"});
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(base).unwrap()
}

#[test]
fn a_named_outcome_is_the_label_and_the_implicit_done_stays_finished() {
    let bad = run(&json!({"verdict": {"name": "check failed", "tone": "bad"}}));
    assert_eq!(label_of(&bad), "check failed");
    assert_eq!(named(&bad).map(|v| v.name.as_str()), Some("check failed"));
    let implicit = run(&json!({"verdict": {"name": "done", "tone": "good"}}));
    assert_eq!(
        (label_of(&implicit), named(&implicit)),
        ("finished".to_owned(), None)
    );
    assert_eq!(label_of(&run(&json!({}))), "finished");
}

#[test]
fn a_run_with_a_bad_outcome_is_not_a_failed_run() {
    let bad = run(&json!({"verdict": {"name": "check failed", "tone": "bad"}}));
    assert!(!super::super::went_wrong(&bad));
    let failed = run(&json!({"outcome": "failed"}));
    assert!(super::super::went_wrong(&failed));
    assert_eq!(label_of(&failed), "failed");
}

fn at(step: &str, phase: &str) -> LastRunView {
    run(&json!({"phase": phase, "outcome": "started", "history": [
        {"at": 1, "entry": {"kind": "entered", "step": "proof", "try_n": 1}},
        {"at": 2, "entry": {"kind": "gate_passed", "step": "proof"}},
        {"at": 3, "entry": {"kind": "entered", "step": step, "try_n": 1}},
    ]}))
}

#[test]
fn the_closing_stage_reads_deciding_then_waiting_and_only_there() {
    assert_eq!(closing(&at("outcome", "running")), Some("deciding"));
    assert_eq!(closing(&at("outcome", "waiting")), Some("waiting"));
    assert_eq!(closing(&at("proof", "running")), None);
    let ended = run(&json!({"verdict": {"name": "nothing to do", "tone": "plain"}}));
    assert_eq!(closing(&ended), None);
    assert_eq!(closing(&run(&json!({"phase": "running"}))), None);
}
