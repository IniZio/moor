//! The repo config tools against the fake controller.

use serde_json::json;

use super::mcp;

#[test]
fn the_repo_config_tools_send_their_requests_as_the_token_s_person() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for (tool, args) in [
        ("get_repo_config", json!({"repo": "acme/web"})),
        (
            "set_repo_config",
            json!({"repo": "acme/web", "text": "setup: [make]\n", "expected_revision": 2}),
        ),
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
                "request": {"cmd": "repo-config-get", "repo": "acme/web"}}),
            json!({"cmd": "as", "identity": who,
                "request": {"cmd": "repo-config-set", "repo": "acme/web",
                    "text": "setup: [make]\n", "expected_revision": 2}}),
        ]
    );
}

#[test]
fn a_read_token_reads_but_cannot_set_a_repo_config() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "set_repo_config",
        json!({"repo": "acme/web", "text": "setup: [make]\n"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"]
        .get(0)
        .and_then(|c| c["text"].as_str())
        .unwrap_or_default();
    assert!(text.contains("moor:write"), "{text}");
    assert!(m.requests().is_empty());
}

#[test]
fn a_repo_that_is_not_owner_slash_name_is_refused_before_the_controller() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(&token, "get_repo_config", json!({"repo": "not a repo"}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(m.requests().is_empty());
}

#[test]
fn the_repo_config_tools_read_and_write_the_rules_when_asked() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let rules = "rules:\n  - hosts: [api.github.com]\n";
    for (tool, args) in [
        (
            "get_repo_config",
            json!({"repo": "acme/web", "file": "rules"}),
        ),
        (
            "set_repo_config",
            json!({"repo": "acme/web", "file": "rules", "text": rules, "expected_revision": 1}),
        ),
    ] {
        let result = m.call(&token, tool, args);
        assert_ne!(result["isError"], true, "{tool}: {result}");
    }
    let sent: Vec<_> = m
        .requests()
        .iter()
        .map(|r| r["request"]["request"].clone())
        .collect();
    assert_eq!(
        sent,
        [
            json!({"cmd": "repo-config-get", "repo": "acme/web", "file": "rules"}),
            json!({"cmd": "repo-config-set", "repo": "acme/web", "file": "rules",
                "text": rules, "expected_revision": 1}),
        ]
    );
}

#[test]
fn a_file_other_than_config_or_rules_is_refused_before_the_controller() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "get_repo_config",
        json!({"repo": "acme/web", "file": "secrets"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(m.requests().is_empty());
}
