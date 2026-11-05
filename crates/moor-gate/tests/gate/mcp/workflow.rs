//! The workflow tools against the fake controller.

use serde_json::json;

use super::mcp;

#[test]
fn add_workflow_sends_workflow_add_as_the_token_s_person_and_no_origin() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "add_workflow",
        json!({"name": "daily-fix", "cron": "0 9 * * *", "timezone": "Europe/London",
            "repos": ["acme/web", "acme/api"], "prompt": "fix the alerts",
            "timeout_minutes": 90}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        m.requests()[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "workflow-add", "name": "daily-fix", "cron": "0 9 * * *",
            "timezone": "Europe/London", "repos": ["acme/web", "acme/api"],
            "prompt": "fix the alerts", "timeout_minutes": 90}})
    );
}

#[test]
fn the_other_workflow_tools_send_their_requests() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for (tool, args) in [
        ("list_workflows", json!({})),
        ("remove_workflow", json!({"name": "daily-fix"})),
        (
            "set_workflow_enabled",
            json!({"name": "daily-fix", "enabled": false}),
        ),
        ("list_workflow_runs", json!({"after": 12, "limit": 5})),
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
            json!({"cmd": "workflow-list"}),
            json!({"cmd": "workflow-remove", "name": "daily-fix"}),
            json!({"cmd": "workflow-set-enabled", "name": "daily-fix", "enabled": false}),
            json!({"cmd": "workflow-runs", "after": 12, "limit": 5}),
        ]
    );
}

#[test]
fn a_read_token_reads_but_cannot_change_workflows() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    for (tool, args) in [
        (
            "add_workflow",
            json!({"name": "n", "cron": "0 9 * * *", "timezone": "UTC", "repos": ["o/n"],
                "prompt": "p"}),
        ),
        ("remove_workflow", json!({"name": "n"})),
        (
            "set_workflow_enabled",
            json!({"name": "n", "enabled": true}),
        ),
    ] {
        let result = m.call(&token, tool, args);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("moor:write"), "{text}");
    }
    assert!(m.requests().is_empty());
    for tool in ["list_workflows", "list_workflow_runs"] {
        let result = m.call(&token, tool, json!({}));
        assert_ne!(result["isError"], true, "{tool}: {result}");
    }
}

#[test]
fn the_write_tools_say_a_run_acts_as_the_caller_and_may_open_pull_requests() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let text = tools
        .iter()
        .find(|t| t["name"] == "add_workflow")
        .and_then(|t| t["description"].as_str())
        .unwrap();
    for want in ["acts as you", "pull requests", "cannot merge"] {
        assert!(text.contains(want), "{text}");
    }
}
