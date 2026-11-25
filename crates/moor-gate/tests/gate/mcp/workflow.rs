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
fn run_workflow_sends_its_pull_request_and_refuses_one_it_cannot_read() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let bad = m.call(
        &token,
        "run_workflow",
        json!({"name": "daily-fix", "pull": "acme/web"}),
    );
    assert_eq!(bad["isError"], true, "{bad}");
    assert!(m.requests().is_empty(), "nothing reaches the controller");
    let result = m.call(
        &token,
        "run_workflow",
        json!({"name": "daily-fix", "pull": "acme/web#65"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        m.requests()[0]["request"]["request"],
        json!({"cmd": "workflow-run-now", "name": "daily-fix", "pull": "acme/web#65"})
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
fn add_workflow_refuses_a_plan_gates_or_outcomes_and_sends_nothing() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for field in ["plan", "gates", "outcomes"] {
        let mut args = json!({"name": "daily", "repos": ["acme/web"], "prompt": "fix it"});
        args[field] = json!([]);
        let reply = m.rpc(&token, "tools/call", json!({"name": "add_workflow", "arguments": args}));
        let refused = reply.get("error").is_some() || reply["result"]["isError"] == true;
        assert!(refused, "{field}: {reply}");
        assert!(reply.to_string().contains(field), "{field}: {reply}");
    }
    assert!(m.requests().is_empty(), "nothing reaches the controller");
}

#[test]
fn the_add_workflow_text_says_moor_reads_the_prompt_and_no_plan_is_sent() {
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
    for gone in ["\"plan\"", "\"gates\"", "\"outcomes\""] {
        assert!(!schema.contains(gone), "{gone} still in {schema}");
    }
    let text = tool["description"].as_str().unwrap();
    for want in ["Write the prompt only", "is refused", "expected_revision"] {
        assert!(text.contains(want), "{want} missing from {text}");
    }
}

#[test]
fn add_workflow_enabled_false_and_the_runs_filter_reach_the_controller() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let added = m.call(
        &token,
        "add_workflow",
        json!({"name": "held", "cron": "0 9 * * *", "timezone": "UTC", "repos": ["o/n"],
            "prompt": "p", "enabled": false}),
    );
    assert_ne!(added["isError"], true, "{added}");
    let listed = m.call(&token, "list_workflow_runs", json!({"workflow": "held"}));
    assert_ne!(listed["isError"], true, "{listed}");
    let sent: Vec<_> = m
        .requests()
        .iter()
        .map(|r| r["request"]["request"].clone())
        .collect();
    assert_eq!(sent[0]["enabled"], false);
    assert_eq!(sent[1], json!({"cmd": "workflow-runs", "workflow": "held"}));
}

#[test]
fn list_workflow_runs_sends_its_pull_request_and_refuses_one_it_cannot_read() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let args = json!({"workflow": "held", "pull": "o/n#7", "after": 3});
    let listed = m.call(&token, "list_workflow_runs", args);
    assert_ne!(listed["isError"], true, "{listed}");
    let bad = m.call(&token, "list_workflow_runs", json!({"pull": "o/n"}));
    assert_eq!(bad["isError"], true, "{bad}");
    let sent: Vec<_> = m
        .requests()
        .iter()
        .map(|r| r["request"]["request"].clone())
        .collect();
    assert_eq!(
        sent,
        [json!({"cmd": "workflow-runs", "after": 3, "workflow": "held", "pull": "o/n#7"})],
        "the unreadable pull request never reaches the controller"
    );
}

#[test]
fn the_tool_texts_say_repo_expands_to_every_box_repo_and_how_to_add_disabled() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let text = tools
        .iter()
        .find(|t| t["name"] == "add_workflow")
        .and_then(|t| t["description"].as_str())
        .unwrap();
    for want in ["every repo of the Box", "enabled: false"] {
        assert!(text.contains(want), "{text}");
    }
}

#[test]
fn add_workflow_sends_only_what_the_caller_left_in_and_null_cron_clears_it() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for args in [
        json!({"name": "w", "repos": ["o/n"], "prompt": "p"}),
        json!({"name": "w", "repos": ["o/n"], "prompt": "p", "cron": null}),
        json!({"name": "w", "repos": ["o/n"], "prompt": "p", "config": "",
            "on_pull_request": false, "enabled": true}),
    ] {
        let result = m.call(&token, "add_workflow", args);
        assert_ne!(result["isError"], true, "{result}");
    }
    let sent: Vec<_> = m
        .requests()
        .iter()
        .map(|r| r["request"]["request"].clone())
        .collect();
    assert_eq!(
        sent[0],
        json!({"cmd": "workflow-add", "name": "w", "repos": ["o/n"], "prompt": "p"}),
        "nothing left out is sent as a value"
    );
    assert_eq!(sent[1]["cron"], "", "null clears the cron");
    assert_eq!(sent[2]["config"], "");
    assert_eq!(sent[2]["on_pull_request"], false);
    assert_eq!(sent[2]["enabled"], true);
}
