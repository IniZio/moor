//! The tools the gate lists.

use serde_json::json;

use super::mcp;

#[test]
fn the_tools_are_listed() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let mut names: Vec<&str> = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "add_webhook",
            "add_workflow",
            "box_status",
            "close_box",
            "close_failed_boxes",
            "get_artifact",
            "inspect_box",
            "list_boxes",
            "list_events",
            "list_nodes",
            "list_webhooks",
            "list_workflow_runs",
            "list_workflows",
            "remove_webhook",
            "remove_workflow",
            "retry_box",
            "send_to_box",
            "set_workflow_enabled",
            "start_box"
        ]
    );
}
