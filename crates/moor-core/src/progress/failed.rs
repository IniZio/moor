//! Whether a Box failed: the one answer every front reads, never guessed from error entries.

use serde_json::Value;

use crate::view::EventView;

/// Event kinds that end a Box's work badly.
const FAILURES: &[&str] = &[
    "setup_failed",
    "rejected",
    "session_failed",
    "relaunch_capped",
    "death_capped",
    "run_failed",
];

/// The failure of a closed Box that no later retry or relaunch cleared.
pub(super) fn closed_failure(events: &[EventView]) -> Option<&EventView> {
    let mut failure = None;
    for e in events {
        if FAILURES.contains(&e.kind.as_str()) || died(e) {
            failure = Some(e);
        } else if recovers(e) {
            failure = None;
        }
    }
    failure
}

/// A session that ended on its own with a nonzero exit.
fn died(e: &EventView) -> bool {
    e.kind == "session_ended"
        && e.payload.get("cause").and_then(Value::as_str) == Some("exited")
        && e.payload
            .get("exit")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
}

/// A new session or a passed setup.
fn recovers(e: &EventView) -> bool {
    match e.kind.as_str() {
        "session_started" => true,
        "setup_finished" => e.payload.get("ok").and_then(Value::as_bool) != Some(false),
        _ => false,
    }
}
