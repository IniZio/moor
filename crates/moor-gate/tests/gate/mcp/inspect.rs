//! `inspect_box` against the fake controller.

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::mcp;

/// The reply the fake controller gives without further checks: an `inspect` answer or a
/// webhook refusal; none for any other request.
pub(super) fn early(cmd: &str, inner: &Value, id: &Value) -> Option<Value> {
    answer(cmd, inner, id).or_else(|| super::webhook::refusal(cmd, inner, id))
}

/// The controller's answer to an `inspect` request frame `id`: a view, or "no Box" for the
/// Box `repo/missing`; none when `cmd` is not `inspect`.
fn answer(cmd: &str, inner: &Value, id: &Value) -> Option<Value> {
    if cmd != "inspect" {
        return None;
    }
    let envelope = if inner["box"] == "repo/missing" {
        let error = ErrorView {
            kind: ErrorKind::Failed,
            message: "no Box repo/missing".to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        Response::err("inspect", error)
    } else {
        let view = json!({"box": inner["box"], "lifecycle": "ready", "session": "working",
            "agent": null, "pane": {"tail": "$ cargo test\nrunning", "status": "live"},
            "said": [{"text": "running the tests", "at": "2026-10-29T10:00:00Z"}],
            "stuck": {"kind": "stalled", "since": "2026-10-29T10:20:00Z",
                "detail": "no hook report for 12 min while Bash cargo test"}});
        Response::ok("inspect", Some(view))
    };
    Some(json!({"frame": "response", "id": id, "envelope": envelope}))
}

#[test]
fn inspect_box_is_listed_with_its_three_arguments() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let tool = tools.iter().find(|t| t["name"] == "inspect_box").unwrap();
    let schema = &tool["inputSchema"];
    let mut args: Vec<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    args.sort_unstable();
    assert_eq!(args, ["box", "lines", "said"]);
    assert_eq!(schema["required"], json!(["box"]));
}

#[test]
fn inspect_box_forwards_the_request_as_the_token_s_person() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "inspect_box",
        json!({"box": "repo/web", "lines": 80, "said": 3}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["stuck"]["kind"], "stalled");
    assert_eq!(result["structuredContent"]["pane"]["status"], "live");
    assert_eq!(
        m.requests()[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "inspect", "box": "repo/web", "lines": 80, "said": 3}})
    );
}

#[test]
fn inspect_box_leaves_out_what_it_was_not_given() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    m.call(&token, "inspect_box", json!({"box": "repo/web"}));
    assert_eq!(
        m.requests()[0]["request"]["request"],
        json!({"cmd": "inspect", "box": "repo/web"})
    );
}

#[test]
fn a_token_without_the_read_scope_cannot_inspect() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:write");
    let result = m.call(&token, "inspect_box", json!({"box": "repo/web"}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("moor:read"),
        "{result}"
    );
    assert!(m.requests().is_empty());
}

#[test]
fn an_unknown_box_comes_back_as_a_tool_error() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(&token, "inspect_box", json!({"box": "repo/missing"}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no Box repo/missing"),
        "{result}"
    );
}
