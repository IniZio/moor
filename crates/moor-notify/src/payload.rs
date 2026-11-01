//! The JSON body posted for one event.

use moor_client::EventRecord;
use serde_json::{json, Map, Value};

/// The `schema` every body names; a change to the shape gets a new one.
pub const SCHEMA: &str = "moor.notify/1";

/// The body for `event`.
///
/// `shown` is the controller's `show` of the event's Box, when it answered. Its lifecycle,
/// Session state and the agent's newest message are copied in, the same fields
/// `box_status` returns.
#[must_use]
pub fn body(event: &EventRecord, shown: Option<&Value>) -> Value {
    let mut body = json!({
        "schema": SCHEMA,
        "delivery": event.cursor,
        "event": event.kind,
        "ts": event.ts,
        "box": event.box_name,
        "box_id": event.box_id.map(moor_core::BoxId::get),
        "session": event.session.map(moor_core::SessionId::get),
        "data": event.payload,
    });
    let boxed = shown.and_then(|v| v.get("box"));
    if let (Some(boxed), Some(out)) = (boxed, body.as_object_mut()) {
        copy(boxed, out, "lifecycle", "lifecycle");
        copy(boxed, out, "session_state", "session_state");
        copy(boxed, out, "said", "agent_said");
    }
    body
}

/// Copies `from[key]` into `out[as_key]` when it is there.
fn copy(from: &Value, out: &mut Map<String, Value>, key: &str, as_key: &str) {
    if let Some(value) = from.get(key).filter(|v| !v.is_null()) {
        out.insert(as_key.to_owned(), value.clone());
    }
}

#[cfg(test)]
mod tests;
