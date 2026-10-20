//! End to end: moor-web against a real `moor controller`, with no provider.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed setup step should abort the test; indexing a JSON value yields null, not a panic"
)]

mod support;

use serde_json::{json, Value};
use support::Stack;

/// Parses a JSON reply body.
fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not json ({e}): {body}"))
}

/// The root in `/tree` whose request carries `key`.
fn root_for(stack: &Stack, key: &str) -> Option<Value> {
    let (status, body) = stack.get("/tree");
    assert_eq!(status, 200, "GET /tree: {body}\n{}", stack.diagnostics());
    parse(&body)["roots"]
        .as_array()?
        .iter()
        .find(|r| r["request"]["key"] == key)
        .cloned()
}

/// Controller config naming one repo, so the repo check passes; the clone then fails offline.
const DEMO_REPO: &str = "repos:\n  demo:\n    url: /nonexistent/moor-e2e/demo.git\n";

/// Posts a new request and returns its key after checking the job failed only at `start`.
fn create_request(stack: &Stack) -> String {
    let (status, body) = stack.post(
        "/requests",
        &json!({"repo": "demo", "name": "t1", "goal": "g"}),
    );
    assert_eq!(
        status,
        202,
        "POST /requests: {body}\n{}",
        stack.diagnostics()
    );
    let accepted = parse(&body);
    let key = accepted["key"].as_str().expect("key").to_owned();
    let job = accepted["job"].as_str().expect("job").to_owned();

    // The repo is known, so only the worker start can fail.
    let snap = stack.wait_job(&job);
    let diag = stack.diagnostics();
    assert_eq!(snap["state"], "failed", "job: {snap}\n{diag}");
    assert_eq!(snap["failure"]["step"], "start", "job: {snap}\n{diag}");
    let message = snap["failure"]["message"].as_str().unwrap_or_default();
    assert!(!message.is_empty(), "job: {snap}\n{diag}");
    let progress = snap["progress"].to_string();
    for line in [
        "creating the request workdir",
        "opening the request session",
        "binding the request",
        "saving the request",
        "starting the worker",
    ] {
        assert!(progress.contains(line), "no `{line}` in {progress}");
    }
    key
}

/// Checks the open root in the tree, the binding and the Session; returns the root id.
fn check_open_root(stack: &Stack, key: &str) -> i64 {
    let diag = stack.diagnostics();
    let root = root_for(stack, key).unwrap_or_else(|| panic!("no root for key\n{diag}"));
    let id = root["session"].as_i64().expect("root session id");
    assert_eq!(root["name"], Value::Null, "root has a Box: {root}");
    assert_eq!(root["request"]["repo"], "demo");
    assert_eq!(root["request"]["ended_at"], Value::Null);
    assert_ne!(root["state"], "gone", "root: {root}");

    let bound = stack.controller(json!({"cmd": "session-binding", "front": "web", "key": key}));
    let binding = &bound["value"]["binding"];
    let bound_session = if binding.is_null() {
        &bound["value"]["session"]
    } else {
        &binding["session"]
    };
    assert_eq!(bound_session, &json!(id), "binding: {bound}\n{diag}");

    let shown = stack.controller(json!({"cmd": "show-session", "session": id}));
    assert_eq!(shown["value"]["box"], Value::Null, "show: {shown}");
    assert_eq!(shown["value"]["ended_at"], Value::Null, "show: {shown}");
    id
}

/// Ends the request and checks the root ends, the tree shows it gone and workers are refused.
fn end_request(stack: &Stack, key: &str, id: i64) {
    let (status, body) = stack.post(&format!("/requests/{key}/end"), &json!({}));
    assert_eq!(status, 202, "end: {body}\n{}", stack.diagnostics());
    let end_job = parse(&body)["job"].as_str().expect("job").to_owned();
    let snap = stack.wait_job(&end_job);
    let diag = stack.diagnostics();
    assert_eq!(snap["state"], "done", "end job: {snap}\n{diag}");
    assert_eq!(snap["result"]["root_ended"], true, "end job: {snap}");

    let shown = stack.controller(json!({"cmd": "show-session", "session": id}));
    let listed = stack.controller(json!({"cmd": "sessions"}));
    let still_live = listed["value"]["sessions"]
        .as_array()
        .is_some_and(|s| s.iter().any(|r| r["session"] == json!(id)));
    assert!(
        shown["value"]["ended_at"].is_i64() || !still_live,
        "root still live: {shown}\n{diag}"
    );

    let gone = root_for(stack, key).unwrap_or_else(|| panic!("root vanished\n{diag}"));
    assert_eq!(gone["state"], "gone", "root: {gone}\n{diag}");
    assert!(gone["request"]["ended_at"].is_i64(), "unset: {gone}");

    let (status, body) = stack.post(
        &format!("/requests/{key}/workers"),
        &json!({"name": "t2", "goal": "g"}),
    );
    assert_eq!(status, 409, "workers after end: {body}\n{diag}");
}

#[test]
fn request_lifecycle_against_a_real_controller() {
    let stack = Stack::start_with(DEMO_REPO);
    let (status, page) = stack.get("/");
    assert_eq!(status, 200, "GET /\n{}", stack.diagnostics());
    assert!(page.contains("/app.js"), "index lacks /app.js");

    let key = create_request(&stack);
    let id = check_open_root(&stack, &key);
    end_request(&stack, &key, id);
}

#[test]
fn unknown_repo_fails_before_any_root_is_opened() {
    let stack = Stack::start();
    let (status, body) = stack.post(
        "/requests",
        &json!({"repo": "demo", "name": "t1", "goal": "g"}),
    );
    assert_eq!(status, 202, "POST /requests: {body}");
    let accepted = parse(&body);
    let key = accepted["key"].as_str().expect("key").to_owned();
    let job = accepted["job"].as_str().expect("job").to_owned();

    let snap = stack.wait_job(&job);
    let diag = stack.diagnostics();
    assert_eq!(snap["state"], "failed", "job: {snap}\n{diag}");
    assert_eq!(snap["failure"]["step"], "repo", "job: {snap}\n{diag}");
    assert_eq!(
        snap["failure"]["message"],
        "repo demo is not known to the controller; no repos are configured",
        "job: {snap}\n{diag}"
    );

    let listed = stack.controller(json!({"cmd": "sessions"}));
    let sessions = listed["value"]["sessions"].as_array().expect("sessions");
    assert!(sessions.is_empty(), "sessions: {listed}\n{diag}");

    let bound = stack.controller(json!({"cmd": "session-binding", "front": "web", "key": key}));
    assert_eq!(
        bound["value"],
        Value::Null,
        "binding exists: {bound}\n{diag}"
    );

    assert!(root_for(&stack, &key).is_none(), "root exists\n{diag}");
}
