//! `exec_in_box` against the fake controller.

use std::io::Write;

use moor_core::controller::{write_bytes, write_stderr};
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::mcp;

/// Answers an `exec` request `frame` on `out`: the command's output frames, then how it
/// ended. `loud` writes more stdout than the tool keeps, `sleep 300` runs past its timeout,
/// and the Box `repo/missing` is not the caller's. A frame that is no request, such as the
/// eof that closes the command's stdin, gets no answer. False for any other request.
pub(super) fn answer(frame: &Value, out: &mut dyn Write) -> bool {
    if frame["frame"] != "request" {
        return true;
    }
    let (inner, id) = (&frame["request"]["request"], &frame["id"]);
    if inner["cmd"] != "exec" {
        return false;
    }
    let id_text = id.as_str().unwrap_or_default();
    let envelope = if inner["box"] == "repo/missing" {
        Response::err("exec", missing())
    } else {
        let (stdout, stderr, view) = match inner["argv"][0].as_str().unwrap_or_default() {
            "loud" => (vec![b'a'; 70 * 1024], b"".to_vec(), json!({"exit": 0})),
            "sleep 300" => (b"".to_vec(), b"".to_vec(), json!({"timed_out": true})),
            _ => (b"hello\n".to_vec(), b"oops\n".to_vec(), json!({"exit": 3})),
        };
        write_bytes(out, id_text, &stdout).unwrap();
        write_stderr(out, id_text, &stderr).unwrap();
        Response::ok("exec", Some(view))
    };
    let frame = json!({"frame": "response", "id": id, "envelope": envelope});
    writeln!(out, "{frame}").unwrap();
    true
}

/// The controller's refusal for a Box that is not the caller's or does not exist.
fn missing() -> ErrorView {
    ErrorView {
        kind: ErrorKind::Refused,
        message: "not your Box".to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    }
}

#[test]
fn exec_in_box_is_listed_with_its_three_arguments() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let tool = tools.iter().find(|t| t["name"] == "exec_in_box").unwrap();
    let schema = &tool["inputSchema"];
    let mut args: Vec<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    args.sort_unstable();
    assert_eq!(args, ["box", "command", "timeout_seconds"]);
    let mut required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["box", "command"]);
}

#[test]
fn exec_in_box_runs_the_command_as_the_token_s_person_and_returns_its_output() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:write");
    let result = m.call(
        &token,
        "exec_in_box",
        json!({"box": "repo/web", "command": "cat /proc/meminfo"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"],
        json!({"exit": 3, "timed_out": false, "stdout": "hello\n", "stderr": "oops\n",
            "stdout_truncated": false, "stderr_truncated": false})
    );
    assert_eq!(
        m.requests()[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "exec", "box": "repo/web", "argv": ["cat /proc/meminfo"],
            "timeout_secs": 30}})
    );
}

#[test]
fn a_timeout_past_the_cap_is_held_to_it() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:write");
    m.call(
        &token,
        "exec_in_box",
        json!({"box": "repo/web", "command": "df -h", "timeout_seconds": 3600}),
    );
    assert_eq!(m.requests()[0]["request"]["request"]["timeout_secs"], 120);
}

#[test]
fn output_past_the_cap_is_cut_and_flagged() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:write");
    let result = m.call(
        &token,
        "exec_in_box",
        json!({"box": "repo/web", "command": "loud"}),
    );
    let got = &result["structuredContent"];
    assert_eq!(got["stdout"].as_str().unwrap().len(), 64 * 1024, "{got}");
    assert_eq!(got["stdout_truncated"], true);
    assert_eq!(got["stderr_truncated"], false);
    assert_eq!(got["exit"], 0);
}

#[test]
fn a_command_that_ran_too_long_says_it_timed_out() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:write");
    let result = m.call(
        &token,
        "exec_in_box",
        json!({"box": "repo/web", "command": "sleep 300"}),
    );
    assert_eq!(result["structuredContent"]["timed_out"], true, "{result}");
    assert_eq!(result["structuredContent"]["exit"], Value::Null);
}

#[test]
fn a_token_without_the_write_scope_cannot_exec() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "exec_in_box",
        json!({"box": "repo/web", "command": "ps"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("moor:write"),
        "{result}"
    );
    assert!(m.requests().is_empty());
}

#[test]
fn a_box_that_is_not_yours_comes_back_as_a_tool_error() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:write");
    let result = m.call(
        &token,
        "exec_in_box",
        json!({"box": "repo/missing", "command": "ps"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not your Box"),
        "{result}"
    );
}
