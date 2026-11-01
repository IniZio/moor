//! The webhook tools against the fake controller.

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::mcp;

/// The one webhook `webhook-list` answers, failing since a while.
pub(super) fn listed() -> Value {
    json!({"webhooks": [{"id": 7, "url": "https://hooks.example/moor",
        "events": ["box_ready"], "created_at": 1, "last_ok_at": null,
        "failing_since": 5000, "last_error": "http 500"}]})
}

/// The controller's refusal of a `webhook-add` whose url is not https, if `cmd` is one.
pub(super) fn refusal(cmd: &str, inner: &Value, id: &Value) -> Option<Value> {
    if cmd != "webhook-add" || inner["url"].as_str()?.starts_with("https://") {
        return None;
    }
    let error = ErrorView {
        kind: ErrorKind::Usage,
        message: "a webhook url must start with https://".to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    };
    Some(json!({"frame": "response", "id": id, "envelope": Response::err("webhook-add", error)}))
}

#[test]
fn add_webhook_sends_webhook_add_and_shows_the_key_once() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "add_webhook",
        json!({"url": "https://hooks.example/moor", "events": ["box_ready"]}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(&"ab".repeat(32)), "{text}");
    assert!(text.contains("only this once"), "{text}");
    assert!(text.contains("X-Moor-Signature"), "{text}");
    assert_eq!(result["structuredContent"]["id"], 7);
    let sent = m.requests();
    assert_eq!(
        sent[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "webhook-add", "url": "https://hooks.example/moor",
            "events": ["box_ready"]}})
    );
}

#[test]
fn list_webhooks_shows_health_and_never_a_key() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(&token, "list_webhooks", json!({}));
    assert_ne!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    for want in ["7", "https://hooks.example/moor", "box_ready", "http 500"] {
        assert!(text.contains(want), "{text}");
    }
    assert!(!text.contains("key"), "{text}");
    assert_eq!(
        m.requests()[0]["request"]["request"],
        json!({"cmd": "webhook-list"})
    );
}

#[test]
fn remove_webhook_sends_webhook_remove() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(&token, "remove_webhook", json!({"id": 7}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        m.requests()[0]["request"]["request"],
        json!({"cmd": "webhook-remove", "id": 7})
    );
}

#[test]
fn a_read_token_cannot_add_or_remove_a_webhook() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    for (tool, args) in [
        ("add_webhook", json!({"url": "https://hooks.example/moor"})),
        ("remove_webhook", json!({"id": 7})),
    ] {
        let result = m.call(&token, tool, args);
        assert_eq!(result["isError"], true, "{result}");
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("moor:write"),
            "{result}"
        );
    }
    assert!(m.requests().is_empty());
}

#[test]
fn a_refused_webhook_comes_back_as_a_tool_error_with_the_reason() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "add_webhook",
        json!({"url": "http://plain.example"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("must start with https://"),
        "{result}"
    );
}
