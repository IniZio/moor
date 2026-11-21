//! The sentences of the lines only a gated run writes.

use serde_json::Value;

use super::text;

/// The sentence for a line a gated run writes; any other kind reads as the line it was stored as.
pub(super) fn sentence(kind: &str, entry: &Value, tries: &str) -> String {
    match kind {
        "gate_passed" => "Gate passed".to_owned(),
        "gate_waiting" => text(entry, "why").map_or_else(
            || format!("Waiting for the owner's answer{tries}"),
            |why| format!("Waiting for a person{tries}: {why}"),
        ),
        "gate_answered" => {
            let answer = text(entry, "answer").unwrap_or("?");
            let note = text(entry, "note").filter(|n| !n.is_empty());
            note.map_or_else(
                || format!("Owner answered {answer}{tries}"),
                |n| format!("Owner answered {answer}{tries}: {n}"),
            )
        }
        "resumed" => "Resumed after the wait".to_owned(),
        other => format!("{other}: {entry}"),
    }
}
