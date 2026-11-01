//! What the webhook tools add to the controller's answers so a model can tell a person how
//! their webhooks are doing.

use moor_core::view::progress;
use serde_json::{json, Value};

/// The state a webhook waits for its person in.
const WAITING: &str = "waiting_for_secret";

/// Where the person sets up webhook `id` on the gate at `origin`.
fn setup_url(origin: &str, id: &Value) -> String {
    format!("{origin}/webhooks/{id}/setup")
}

/// Gives the new webhook's setup link, when the gate at `origin` serves one, and says plainly
/// that it delivers nothing until its person sets it up there.
pub(super) fn added(value: &mut Value, origin: Option<&str>) {
    let link = origin.zip(value.get("id")).map(|(o, id)| setup_url(o, id));
    let how = if link.is_some() {
        "Have its person open setup_url in a browser to see the signing key and enter the \
         header value."
    } else {
        "Setup needs the gate's browser sign-in, which this gate does not serve."
    };
    let note = format!(
        "The webhook is waiting for its secret and delivers nothing until it is set up. {how} \
         Never paste secrets into this chat."
    );
    if let Some(reply) = value.as_object_mut() {
        if let Some(link) = link {
            reply.insert("setup_url".to_owned(), json!(link));
        }
        reply.insert("note".to_owned(), json!(note));
    }
}

/// Adds a `health` line to each webhook in `value`, with `now` in unix seconds, and a
/// `setup_url` on the gate at `origin` to each one still waiting.
pub(super) fn listed(value: &mut Value, origin: Option<&str>, now: i64) {
    let Some(Value::Array(rows)) = value.get_mut("webhooks") else {
        return;
    };
    for row in rows {
        let waiting = row.get("state").and_then(Value::as_str) == Some(WAITING);
        let line = if waiting {
            "waiting for secret; nothing is delivered until it is set up".to_owned()
        } else {
            format!("active; {}", health(row, now))
        };
        let setup = origin
            .zip(row.get("id"))
            .filter(|_| waiting)
            .map(|(o, id)| setup_url(o, id));
        if let Some(row) = row.as_object_mut() {
            row.insert("health".to_owned(), json!(line));
            if let Some(setup) = setup {
                row.insert("setup_url".to_owned(), json!(setup));
            }
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
