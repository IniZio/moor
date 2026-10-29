use serde_json::{json, Value};

use super::{failure, Happened, Progress};

const T0: i64 = 1_793_000_000;

fn event(kind: &str, payload: Value, secs: i64) -> Happened {
    let at = time_at(T0 + secs);
    Happened {
        kind: kind.to_owned(),
        payload,
        ts: at,
    }
}

/// `unix` as the controller writes it.
fn time_at(unix: i64) -> String {
    let (days, rem) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    assert_eq!(days, 20_752, "the fixture stays on 2026-10-26");
    format!(
        "2026-10-26T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[test]
fn each_setup_step_reads_as_words_with_its_time_since_the_start() {
    let events = [
        event("started", json!({}), 0),
        event("waiting_for_box", json!({"max_boxes": 2}), 0),
        event("box_requested", json!({}), 3),
        event("box_ready", json!({}), 20),
        event("setting_up", json!({"from": "waiting_for_box"}), 21),
        event("cloned", json!({"ms": 12_400}), 33),
        event("setup_finished", json!({"ms": 40_000, "ok": true}), 75),
    ];
    let p = Progress::of(&events, T0 + 80);
    assert_eq!(p.elapsed, Some(80));
    assert_eq!(
        p.steps,
        [
            "    +0s  waiting for a machine slot",
            "    +3s  machine requested",
            "   +20s  machine ready",
            "   +21s  setting up",
            "   +33s  cloned in 12s",
            "+1m 15s  setup finished in 40s",
        ]
    );
}

#[test]
fn failures_say_what_failed() {
    let events = [
        event("started", json!({}), 0),
        event(
            "credential_failed",
            json!({"which": "github", "error": "token expired"}),
            1,
        ),
        event("box_failed", json!({"error": "no capacity"}), 2),
        event(
            "setup_failed",
            json!({"line": "npm ERR! missing script"}),
            3,
        ),
    ];
    let steps = Progress::of(&events, T0 + 3).steps;
    assert_eq!(
        steps,
        [
            "    +1s  credential failed: github: token expired",
            "    +2s  machine failed: no capacity",
            "    +3s  setup failed: npm ERR! missing script",
        ]
    );
}

#[test]
fn only_the_latest_steps_are_kept() {
    let events: Vec<_> = (0..20)
        .map(|i| event("box_requested", json!({}), i))
        .collect();
    let steps = Progress::of(&events, T0 + 20).steps;
    assert_eq!(steps.len(), 8);
    assert!(steps.last().unwrap().starts_with("   +19s"));
}

#[test]
fn a_box_with_no_events_has_no_elapsed_time() {
    assert_eq!(Progress::of(&[], T0), Progress::default());
}

#[test]
fn a_setup_failure_explains_a_down_box_until_it_is_closed() {
    let failed = [
        event("started", json!({}), 0),
        event("setup_failed", json!({"line": "exit 1"}), 5),
        event("idled", json!({}), 6),
    ];
    assert_eq!(failure(&failed).as_deref(), Some("setup failed: exit 1"));
    let mut closed = failed.to_vec();
    closed.push(event("closed", json!({"from": "setup_failed"}), 9));
    assert_eq!(failure(&closed), None);
    assert_eq!(
        failure(&[event("rejected", json!({"reason": "no repo"}), 0)]).as_deref(),
        Some("rejected: no repo")
    );
}
