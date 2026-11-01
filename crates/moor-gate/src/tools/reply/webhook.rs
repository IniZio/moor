//! What the webhook tools add to the controller's answers so a model can tell a person how
//! their webhooks are doing.

use moor_core::view::progress;
use serde_json::{json, Value};

/// Says plainly, next to the new webhook's `key`, that it is shown only once.
pub(super) fn added(value: &mut Value) {
    if let Some(reply) = value.as_object_mut() {
        reply.insert(
            "note".to_owned(),
            json!(
                "This key is shown only this once; save it now. It is the HMAC key that signs \
                 each POST, in the X-Moor-Signature header, so the receiver can check it."
            ),
        );
    }
}

/// Adds a `health` line to each webhook in `value`, with `now` in unix seconds.
pub(super) fn listed(value: &mut Value, now: i64) {
    let Some(Value::Array(rows)) = value.get_mut("webhooks") else {
        return;
    };
    for row in rows {
        let line = health(row, now);
        if let Some(row) = row.as_object_mut() {
            row.insert("health".to_owned(), json!(line));
        }
    }
}

/// How deliveries to the webhook in `row` are going, in words.
fn health(row: &Value, now: i64) -> String {
    let secs = |name: &str| row.get(name).and_then(Value::as_i64).map(|ms| ms / 1000);
    let ago = |at: i64| format!("{} ago", progress::span(now.saturating_sub(at)));
    if let Some(since) = secs("failing_since") {
        let why = row
            .get("last_error")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return format!(
            "failing for {}: {why}",
            progress::span(now.saturating_sub(since))
        );
    }
    secs("last_ok_at").map_or_else(
        || "no delivery yet".to_owned(),
        |at| format!("working; last delivered {}", ago(at)),
    )
}
