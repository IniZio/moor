//! The place tools on the Box-facing listener: each call reaches the fake controller for the Box
//! the Proxy named, and its answer reaches the agent.

use serde_json::{json, Value};

use super::{listener, text, Listener};

impl Listener {
    /// `tool` called with `args` as Box `box_header`.
    fn call(&self, box_header: Option<&str>, tool: &str, args: Value) -> Value {
        let reply = self.rpc(
            box_header,
            "tools/call",
            json!({"name": tool, "arguments": args}),
        );
        reply["result"].clone()
    }
}

#[test]
fn exec_in_box_sends_the_stamped_box_and_no_identity() {
    let l = listener(true);
    let result = l.call(
        Some("41"),
        "exec_in_box",
        json!({"box": "w", "command": "ls", "timeout_seconds": 1800}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let out: Value = serde_json::from_str(text(&result)).unwrap();
    assert_eq!(out["stdout"], "hi\n", "{out}");
    assert_eq!(out["exit"], 0, "{out}");
    assert_eq!(
        l.seen.recv().unwrap(),
        json!({"cmd": "box-tool", "from": 41, "call": {
            "tool": "exec", "target": "w", "command": "ls", "timeout": 1800}})
    );
    l.call(
        Some("41"),
        "exec_in_box",
        json!({"box": "w", "command": "ls"}),
    );
    assert_eq!(l.seen.recv().unwrap()["call"]["timeout"], 30);
}

#[test]
fn a_long_exec_answers_with_its_run_and_exec_wait_reads_its_end() {
    let l = listener(true);
    let result = l.call(
        Some("41"),
        "exec_in_box",
        json!({"box": "w", "command": "sleep"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let out: Value = serde_json::from_str(text(&result)).unwrap();
    assert_eq!(
        (out["state"].clone(), out["run"].clone()),
        (json!("running"), json!(7)),
        "{out}"
    );
    assert_eq!(out["stdout"], "started\n", "{out}");
    l.seen.recv().unwrap();
    let result = l.call(Some("41"), "exec_wait", json!({"run": 7}));
    assert_ne!(result["isError"], true, "{result}");
    let end: Value = serde_json::from_str(text(&result)).unwrap();
    assert_eq!(
        (end["state"].clone(), end["exit"].clone()),
        (json!("exited"), json!(0)),
        "{end}"
    );
    assert_eq!(end["stdout"], "done\n", "{end}");
    assert_eq!(
        l.seen.recv().unwrap(),
        json!({"cmd": "box-tool", "from": 41, "call": {"tool": "wait", "run": 7}})
    );
}

#[test]
fn start_box_sends_the_stamped_box_and_its_repos() {
    let l = listener(true);
    let result = l.call(
        Some("41"),
        "start_box",
        json!({"name": "n", "repos": ["o/a", "o/b"]}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        l.seen.recv().unwrap(),
        json!({"cmd": "box-tool", "from": 41, "call": {
            "tool": "start", "name": "n", "repo": "o/a", "also": ["o/b"]}})
    );
    l.call(
        Some("41"),
        "start_box",
        json!({"name": "n", "repos": ["o/a"], "base": "release-2"}),
    );
    assert_eq!(
        l.seen.recv().unwrap(),
        json!({"cmd": "box-tool", "from": 41, "call": {
            "tool": "start", "name": "n", "repo": "o/a", "base": "release-2"}})
    );
}

#[test]
fn close_box_sends_the_stamped_box_and_the_target_with_no_force() {
    let l = listener(true);
    let result = l.call(Some("41"), "close_box", json!({"box": "w"}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        l.seen.recv().unwrap(),
        json!({"cmd": "box-tool", "from": 41, "call": {"tool": "close", "target": "w"}})
    );
    let tools = l.rpc(Some("41"), "tools/list", json!({}));
    let close = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "close_box")
        .unwrap();
    let args: Vec<&str> = close["inputSchema"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(args, ["box"], "{close}");
}

#[test]
fn a_place_tool_call_the_gate_cannot_send_reaches_no_controller() {
    let l = listener(true);
    let calls = [
        (None, "exec_in_box", json!({"box": "w", "command": "ls"})),
        (
            Some("7"),
            "exec_in_box",
            json!({"box": "w", "command": "ls", "timeout_seconds": 1801}),
        ),
        (
            Some("7"),
            "exec_in_box",
            json!({"box": "w", "command": "ls", "timeout_seconds": 0}),
        ),
        (Some("7"), "start_box", json!({"name": "n", "repos": []})),
        (
            Some("7"),
            "start_box",
            json!({"name": "n", "repos": ["not a repo"]}),
        ),
        (None, "start_box", json!({"name": "n", "repos": ["o/a"]})),
        (None, "close_box", json!({"box": "w"})),
        (None, "exec_wait", json!({"run": 7})),
        (Some("7"), "exec_wait", json!({"run": "seven"})),
        (Some("7"), "close_box", json!({"box": "Not A Name!"})),
    ];
    for (header, tool, args) in calls {
        let result = l.call(header, tool, args.clone());
        assert_eq!(result["isError"], true, "{tool} {args}: {result}");
    }
    assert!(l.seen.try_recv().is_err());
}

#[test]
fn a_work_box_agent_sees_the_place_tools_and_is_refused_by_the_controller() {
    let l = listener(true);
    for (tool, args) in [
        ("exec_in_box", json!({"box": "w", "command": "ls"})),
        ("start_box", json!({"name": "n", "repos": ["o/a"]})),
        ("close_box", json!({"box": "w"})),
        ("exec_wait", json!({"run": 7})),
    ] {
        let result = l.call(Some("9"), tool, args);
        assert_eq!(result["isError"], true, "{result}");
        assert!(text(&result).contains("this is a work Box"), "{result}");
    }
}

#[test]
fn start_box_refuses_a_prompt_saying_a_work_box_runs_no_agent() {
    let l = listener(true);
    let result = l.call(
        Some("7"),
        "start_box",
        json!({"name": "n", "repos": ["o/a"], "prompt": "fix the typo"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        text(&result),
        "start_box takes no prompt: a work Box runs no agent. Do the work yourself: run its \
         commands in the Box with exec_in_box."
    );
    assert!(l.seen.try_recv().is_err());
}

#[test]
fn a_place_tool_refuses_a_field_it_does_not_take_and_names_it() {
    let l = listener(true);
    for (tool, mut args) in [
        ("exec_in_box", json!({"box": "w", "command": "ls"})),
        ("exec_wait", json!({"run": 7})),
        ("start_box", json!({"name": "n", "repos": ["o/a"]})),
        ("close_box", json!({"box": "w"})),
    ] {
        args["gates"] = json!([]);
        let result = l.call(Some("41"), tool, args);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        assert!(
            text(&result).contains("unknown field `gates`"),
            "{tool}: {result}"
        );
    }
    assert!(l.seen.try_recv().is_err());
}

#[test]
fn the_place_tools_advertise_no_field_they_refuse() {
    let l = listener(true);
    let reply = l.rpc(Some("7"), "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    for (tool, fields) in [
        ("exec_in_box", &["box", "command", "timeout_seconds"][..]),
        ("exec_wait", &["run"][..]),
        ("start_box", &["base", "name", "no_setup", "repos"][..]),
        ("close_box", &["box"][..]),
    ] {
        let schema = &tools.iter().find(|t| t["name"] == tool).unwrap()["inputSchema"];
        let mut keys: Vec<&str> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, fields, "{tool}: {schema}");
        assert_eq!(schema["additionalProperties"], false, "{tool}: {schema}");
    }
}
