//! The Stop button on a Box's status message: its action id and the blocks that carry it.

use serde_json::{json, Value};

/// Action id of the Stop button; its value is the Box's name.
pub(crate) const STOP_ACTION: &str = "stop_box";

/// Blocks for a status message showing `text` with a Stop button for Box `name`.
pub(crate) fn blocks(name: &str, text: &str) -> Value {
    json!([
        {"type": "section", "text": {"type": "mrkdwn", "text": text}},
        {"type": "actions", "elements": [{
            "type": "button",
            "action_id": STOP_ACTION,
            "value": name,
            "style": "danger",
            "text": {"type": "plain_text", "text": "Stop"},
        }]},
    ])
}
