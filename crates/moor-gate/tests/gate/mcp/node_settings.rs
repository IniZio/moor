//! The node settings tools against the fake controller.

use serde_json::json;

use super::mcp;

#[test]
fn the_node_settings_tools_send_their_requests_as_the_token_s_person() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for (tool, args) in [
        ("get_node_settings", json!({"name": "hv"})),
        (
            "set_node_settings",
            json!({"name": "hv", "max_boxes": 2, "labels": ["gpu"], "expected_revision": 1}),
        ),
        ("set_node_settings", json!({"name": "hv", "no_cap": true})),
    ] {
        let result = m.call(&token, tool, args);
        assert_ne!(result["isError"], true, "{tool}: {result}");
    }
    let sent: Vec<_> = m.requests().iter().map(|r| r["request"].clone()).collect();
    let who = "authgear:alice-sub";
    assert_eq!(
        sent,
        [
            json!({"cmd": "as", "identity": who,
                "request": {"cmd": "node-settings-get", "name": "hv"}}),
            json!({"cmd": "as", "identity": who,
                "request": {"cmd": "node-settings-set", "name": "hv", "max_boxes": {"set": 2},
                    "labels": ["gpu"], "expected_revision": 1}}),
            json!({"cmd": "as", "identity": who,
                "request": {"cmd": "node-settings-set", "name": "hv", "max_boxes": "clear"}}),
        ]
    );
}

#[test]
fn a_read_token_reads_but_cannot_set_node_settings() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "set_node_settings",
        json!({"name": "hv", "no_cap": true}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(m.requests().is_empty());
}

#[test]
fn a_cap_and_no_cap_together_or_a_cap_of_zero_are_refused_before_the_controller() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for args in [
        json!({"name": "hv", "max_boxes": 2, "no_cap": true}),
        json!({"name": "hv", "max_boxes": 0}),
    ] {
        let result = m.call(&token, "set_node_settings", args);
        assert_eq!(result["isError"], true, "{result}");
    }
    assert!(m.requests().is_empty());
}
