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
            "repos": ["acme/web", "acme/api", "acme/lib"],
            "prompt": "fix the alerts", "timeout_minutes": 90}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        m.requests()[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "workflow-add", "name": "daily-fix", "cron": "0 9 * * *",
            "timezone": "Europe/London", "repos": ["acme/web", "acme/api", "acme/lib"],
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
        (
            "answer_run",
            json!({"workflow": "daily-fix", "answer": "no", "note": "later"}),
        ),
        (
            "run_workflow",
            json!({"name": "daily-fix", "expected_revision": 3}),
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
            json!({"cmd": "workflow-answer", "workflow": "daily-fix",
                "answer": "no", "note": "later"}),
            json!({"cmd": "workflow-run-now", "name": "daily-fix", "expected_revision": 3}),
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
        ("answer_run", json!({"workflow": "n", "answer": "yes"})),
        ("run_workflow", json!({"name": "n"})),
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

#[test]
fn add_workflow_sends_the_plan_as_json_text_with_the_revision() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let plan = json!({"steps": [
        {"id": "audit", "title": "Audit", "kind": "work", "instructions": "list the alerts",
         "done_when": "listed", "evidence": ["alerts.md"], "ok": "ship"},
        {"id": "ship", "title": "Ship", "kind": "deliver", "instructions": "open the PR",
         "done_when": "the PR exists"}]});
    let result = m.call(
        &token,
        "add_workflow",
        json!({"name": "daily-fix", "cron": "0 9 * * *", "timezone": "UTC",
            "repos": ["acme/web"], "prompt": "fix the alerts",
            "plan": plan, "expected_revision": 3}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let sent = &m.requests()[0]["request"]["request"];
    assert_eq!(sent["expected_revision"], 3);
    let text = sent["plan"]
        .as_str()
        .expect("the plan is sent as JSON text");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).unwrap(),
        plan
    );
}

#[test]
fn the_plan_argument_carries_the_plan_schema_and_the_description_says_how_to_write_one() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tool = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "add_workflow")
        .unwrap();
    let schema = tool["inputSchema"].to_string();
    for want in [
        "\"plan\"",
        "\"steps\"",
        "\"max_tries\"",
        "\"evidence\"",
        "expected_revision",
    ] {
        assert!(schema.contains(want), "{want} missing from {schema}");
    }
    let text = tool["description"].as_str().unwrap();
    for want in [
        "one step per finish line",
        "deliver",
        ".moor/artifacts/",
        "max_tries",
        "keep the ids",
        "get_repo_config",
        "every error is listed at once",
        "expected_revision",
    ] {
        assert!(text.contains(want), "{want} missing from {text}");
    }
}
