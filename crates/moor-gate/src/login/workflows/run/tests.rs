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
