//! `list_workflows`, `get_workflow`, `list_workflow_runs` and `get_workflow_run` against the fake
//! controller, which holds one big workflow and its runs.

use serde_json::{json, Value};

use super::mcp;

/// The history a run keeps, long enough to matter.
fn history() -> Value {
    json!([{"at": 1, "entry": {"kind": "started", "note": "HISTORY-MARKER"}},
        {"at": 2, "entry": {"kind": "finished"}}])
}

/// The run `workflow-list` shows as the latest, and `workflow-run-list` shows in full.
fn run(id: i64) -> Value {
    json!({"id": id, "due_at": 1_793_354_400_000_i64, "outcome": "finished", "phase": "done",
        "trigger": "cron", "result": "RESULT-MARKER opened 3 pull requests",
        "detail": "refused: the plan changed\nDETAIL-MARKER second line",
        "box": "acme-web/weekly-9", "history": history()})
}

/// The full workflow `workflow-list` shows.
pub(super) fn full() -> Value {
    json!({"name": "weekly-deps", "cron": "0 6 * * 1", "timezone": "UTC",
        "repos": ["acme/web", "acme/api"], "config": "CONFIG-MARKER: 1",
        "prompt": "# Weekly dependency bump\nPROMPT-MARKER update every dependency",
        "enabled": true, "timeout_minutes": 180, "next_due": 1_793_959_200_000_i64,
        "plan": {"steps": []}, "plan_stale": false, "revision": 4, "on_pull_request": false,
        "gates": [{"name": "proof", "checks": ["cargo test"]}, {"name": "review"}],
        "outcomes": [{"name": "done"}],
        "last_run": run(9)})
}

/// A workflow with no cron, gates, outcomes, last run or prompt heading.
fn sparse() -> Value {
    json!({"name": "by-hand", "repos": ["acme/lib"], "prompt": "just fix it", "enabled": false,
        "timeout_minutes": 60, "plan_stale": false, "revision": 1, "on_pull_request": true})
}

/// A change of `workflow-runs`.
fn change(seq: i64, id: i64) -> Value {
    let mut change = run(id);
    let fields = change.as_object_mut().unwrap();
    fields.insert("kind".to_owned(), json!("run"));
    fields.insert("seq".to_owned(), json!(seq));
    fields.insert("workflow".to_owned(), json!("weekly-deps"));
    fields.insert("step_title".to_owned(), json!("bump"));
    fields.insert("origin".to_owned(), json!({"front": "slack"}));
    change
}

/// The controller's answer to `cmd`, when this module's tools send it.
pub(super) fn answer(cmd: &str, inner: &Value) -> Option<Value> {
    match cmd {
        "workflow-list" => Some(json!({"workflows": [full(), sparse()]})),
        "workflow-runs" => Some(json!({"changes": [change(1, 8), change(2, 9)], "next": 2})),
        "workflow-run-list" => {
            let mut row = run(inner["run"].as_i64().unwrap_or(9));
            row["plan"] = json!({"steps": ["PLAN-MARKER"]});
            let runs = if inner["run"] == 404 {
                vec![]
            } else {
                vec![row]
            };
            Some(json!({"workflow": inner["workflow"], "runs": runs}))
        }
        _ => None,
    }
}

/// The sorted keys of `row`.
fn keys(row: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// Asserts that none of `gone` appears anywhere in `reply`.
fn assert_leaves_out(reply: &Value, gone: &[&str]) {
    let text = reply.to_string();
    for word in gone {
        assert!(!text.contains(word), "{word} in {text}");
    }
}

/// The tool `name`'s description.
fn description(m: &super::Mcp, token: &str, name: &str) -> String {
    let reply = m.rpc(token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let tool = tools.iter().find(|t| t["name"] == name).unwrap();
    tool["description"].as_str().unwrap().to_owned()
}

/// The values of the first compact workflow row.
fn assert_first_row_values(row: &Value) {
    assert_eq!(row["name"], "weekly-deps");
    assert_eq!(row["cron"], "0 6 * * 1");
    assert_eq!(row["timezone"], "UTC");
    assert_eq!(row["repos"], json!(["acme/web", "acme/api"]));
    assert_eq!(row["gates"], json!(["proof", "review"]));
    assert_eq!(row["title"], "Weekly dependency bump");
    let last = row["last_run"].as_str().unwrap();
    for want in ["2026-10-30", "finished", "acme-web/weekly-9"] {
        assert!(last.contains(want), "{last}");
    }
}

#[test]
fn list_workflows_rows_are_compact_and_leave_out_the_prompt_and_run_detail() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "list_workflows", json!({}));
    let rows = reply["structuredContent"]["workflows"].as_array().unwrap();
    assert_eq!(
        keys(&rows[0]),
        [
            "cron", "enabled", "gates", "last_run", "name", "next_due", "repos", "timezone",
            "title"
        ],
        "{reply}"
    );
    assert_first_row_values(&rows[0]);
    assert_eq!(keys(&rows[1]), ["enabled", "name", "repos"], "{reply}");
    assert_eq!(rows[1]["enabled"], false);
    let text = reply.to_string();
    for gone in [
        "PROMPT-MARKER",
        "CONFIG-MARKER",
        "RESULT-MARKER",
        "HISTORY-MARKER",
        "cargo test",
    ] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
}

#[test]
fn get_workflow_returns_the_whole_current_workflow_and_refuses_an_unknown_name() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "get_workflow", json!({"name": "weekly-deps"}));
    assert_ne!(reply["isError"], true, "{reply}");
    assert_eq!(reply["structuredContent"], full(), "{reply}");
    let none = m.call(&token, "get_workflow", json!({"name": "nope"}));
    assert_eq!(none["isError"], true, "{none}");
    let sent: Vec<_> = m
        .requests()
        .iter()
        .map(|r| r["request"]["request"].clone())
        .collect();
    assert_eq!(sent[0], json!({"cmd": "workflow-list"}));
}

#[test]
fn list_workflow_runs_rows_are_compact_and_leave_out_history_and_result() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "list_workflow_runs", json!({}));
    let out = &reply["structuredContent"];
    assert_eq!(out["next"], 2, "{reply}");
    let rows = out["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        keys(&rows[1]),
        [
            "box",
            "detail",
            "due_at",
            "id",
            "outcome",
            "phase",
            "step_title",
            "trigger",
            "workflow"
        ],
        "{reply}"
    );
    assert_eq!(rows[1]["id"], 9);
    assert_eq!(rows[1]["workflow"], "weekly-deps");
    assert_eq!(rows[1]["outcome"], "finished");
    assert_eq!(rows[1]["box"], "acme-web/weekly-9");
    assert_eq!(rows[1]["due_at"], 1_793_354_400_000_i64);
    assert_eq!(rows[1]["detail"], "refused: the plan changed");
    assert_leaves_out(
        &reply,
        &["HISTORY-MARKER", "RESULT-MARKER", "DETAIL-MARKER"],
    );
}

#[test]
fn get_workflow_run_returns_the_whole_run_and_refuses_an_unknown_one() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let args = json!({"workflow": "weekly-deps", "run": 9});
    let reply = m.call(&token, "get_workflow_run", args);
    assert_ne!(reply["isError"], true, "{reply}");
    let run = &reply["structuredContent"];
    assert_eq!(run["id"], 9);
    assert_eq!(run["workflow"], "weekly-deps");
    assert_eq!(run["history"], history());
    assert_eq!(run["result"], "RESULT-MARKER opened 3 pull requests");
    assert!(run["detail"].as_str().unwrap().contains("DETAIL-MARKER"));
    assert_eq!(run["plan"], json!({"steps": ["PLAN-MARKER"]}));
    let none = m.call(
        &token,
        "get_workflow_run",
        json!({"workflow": "weekly-deps", "run": 404}),
    );
    assert_eq!(none["isError"], true, "{none}");
    let sent: Vec<_> = m
        .requests()
        .iter()
        .map(|r| r["request"]["request"].clone())
        .collect();
    assert_eq!(
        sent[0],
        json!({"cmd": "workflow-run-list", "workflow": "weekly-deps", "run": 9})
    );
}

#[test]
fn the_list_tools_send_agents_to_the_get_tools_for_detail() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let workflows = description(&m, &token, "list_workflows");
    assert!(workflows.contains("get_workflow"), "{workflows}");
    let runs = description(&m, &token, "list_workflow_runs");
    assert!(runs.contains("get_workflow_run"), "{runs}");
    let one = description(&m, &token, "get_workflow_run");
    assert!(one.contains("history"), "{one}");
}
