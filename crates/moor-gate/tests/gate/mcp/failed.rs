//! The tools for Boxes whose setup failed.

use serde_json::json;

use super::mcp;

#[test]
fn a_read_token_cannot_retry_or_bulk_close() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    for (tool, args) in [
        ("retry_box", json!({"box": "web/fix"})),
        ("close_failed_boxes", json!({})),
    ] {
        let result = m.call(&token, tool, args);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("moor:write"), "{tool}: {text}");
    }
    assert!(m.requests().is_empty());
}

#[test]
fn retry_box_sends_a_retry_for_the_named_box() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(&token, "retry_box", json!({"box": "web/fix"}));
    assert_ne!(result["isError"], true, "{result}");
    let sent = m.requests();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "retry", "box": "web/fix"}})
    );
}

#[test]
fn close_failed_boxes_sends_close_failed_with_force_off_unless_set() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for (args, force) in [(json!({}), false), (json!({"force": true}), true)] {
        let result = m.call(&token, "close_failed_boxes", args);
        assert_ne!(result["isError"], true, "{result}");
        let sent = m.requests();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(
            sent[0]["request"],
            json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
                "cmd": "close-failed", "force": force}})
        );
    }
}
