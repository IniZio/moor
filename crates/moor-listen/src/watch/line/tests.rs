//! The lines a workflow run tells.

use serde_json::json;

use super::{death_line, newest_worth, run_line};

#[test]
fn a_failed_and_a_waiting_run_print_the_reason_or_question_and_the_run_page() {
    let failed = json!({"workflow": "deps", "repo": "o/n", "run": 7, "outcome": "incomplete",
        "step": "work", "reason": "the agent stopped", "link": "/workflows/deps/runs/7"});
    assert_eq!(
        run_line("repo/a", "run_failed", Some(&failed)).as_deref(),
        Some(
            "moor: box repo/a run_failed: deps (o/n) incomplete at work: the agent stopped \
             /workflows/deps/runs/7"
        )
    );
    let waiting = json!({"workflow": "deps", "repo": "o/n", "run": 8, "step": "ask",
        "question": "ship it?", "link": "/workflows/deps/runs/8"});
    assert_eq!(
        run_line("repo/a", "run_waiting", Some(&waiting)).as_deref(),
        Some("moor: box repo/a run_waiting: deps (o/n) asks: ship it? /workflows/deps/runs/8")
    );
    assert_eq!(run_line("repo/a", "idled", Some(&waiting)), None);
}

#[test]
fn both_run_kinds_are_worth_a_line() {
    for kind in ["run_waiting", "run_failed"] {
        let events = [json!({"id": 1, "kind": kind})];
        assert!(newest_worth(&events).is_some(), "{kind}");
    }
}

#[test]
fn a_box_whose_machine_died_prints_how_it_ended() {
    let capped = json!({"reason": "the work place exited with code 137"});
    assert_eq!(
        death_line("repo/a", "death_capped", Some(&capped)).as_deref(),
        Some(
            "moor: box repo/a death_capped: its machine stopped and moor did not resume it, \
             wake it or message it to bring it back: the work place exited with code 137"
        )
    );
    assert_eq!(death_line("repo/a", "idled", Some(&capped)), None);
    let events = [json!({"id": 1, "kind": "death_capped"})];
    assert!(newest_worth(&events).is_some());
}
