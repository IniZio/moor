//! Tests for the `show` text form.

use moor_core::Response;
use serde_json::{json, Value};

use super::super::render;

fn task(name: &str, with_box: bool) -> Value {
    json!({
        "id": 1, "name": name, "repo": "/r", "state": "running",
        "branch": "moor/x", "worktree": "/w/x",
        "box": if with_box { json!({
            "state": "running", "provider": "local", "workdir": "/work",
            "egress": "none", "sleep": {}
        }) } else { Value::Null },
        "agent": {"state": "working", "pane": "live"},
        "updated_at": "t"
    })
}

fn show(v: Value) -> String {
    render(&Response::ok("show", Some(v)), false).0
}

#[test]
fn empty_list_says_no_tasks() {
    assert_eq!(show(json!({"tasks": []})), "no tasks\n");
}

#[test]
fn list_one_task() {
    let out = show(json!({"tasks": [task("alpha", true)]}));
    assert_eq!(out, "alpha  running  working  running  /work\n");
}

#[test]
fn list_two_tasks_aligns_columns_and_dashes_missing_box() {
    let out = show(json!({"tasks": [task("a", true), task("longer", false)]}));
    assert_eq!(
        out,
        "a       running  working  running  /work\nlonger  running  working  -        -\n"
    );
}

#[test]
fn detail_with_box() {
    let out = show(json!({"task": task("alpha", true)}));
    assert_eq!(
        out,
        "name:     alpha\nstate:    running\nrepo:     /r\nworktree: /w/x\nbranch:   moor/x\n\
         box:      local running workdir=/work egress=none\nagent:    working pane=live\n"
    );
}

#[test]
fn detail_lists_the_question_and_numbered_options() {
    let mut t = task("alpha", false);
    t.as_object_mut().expect("task is an object").insert(
        "agent".into(),
        json!({
            "state": "needs_input", "pane": "live",
            "question": "Do you want to proceed?",
            "options": ["Yes", "No, and tell Claude what to do differently (esc)"]
        }),
    );
    let out = show(json!({"task": t}));
    assert!(out.contains(
        "agent:    needs_input pane=live\n  question: Do you want to proceed?\n  1. Yes\n  \
         2. No, and tell Claude what to do differently (esc)\n"
    ));
}

#[test]
fn detail_without_box() {
    let out = show(json!({"task": task("alpha", false)}));
    assert!(out.contains("box:      none\n"));
}

#[test]
fn detail_with_events_setup_and_pane() {
    let out = show(json!({
        "task": task("alpha", true),
        "setup": {"ran": true, "ok": true},
        "dotfiles": {"ran": true, "ok": false},
        "events": [{"id": 1, "kind": "started", "payload": {}, "ts": "2026-01-01T00:00:00Z"}],
        "pane": {"tail": "line1\nline2", "status": "live"}
    }));
    assert!(out.contains("setup: ok\ndotfiles: failed\n"));
    assert!(out.contains("events:\n  2026-01-01T00:00:00Z  started\n"));
    assert!(out.ends_with("--- pane ---\nline1\nline2\n"));
}

#[test]
fn json_form_keeps_the_raw_value() {
    let v = json!({"tasks": [task("alpha", true)]});
    let out = crate::cli::render_json::render(&Response::ok("show", Some(v.clone())));
    let parsed: Value = serde_json::from_str(&out).expect("json");
    assert_eq!(parsed.get("value"), Some(&v));
}

#[test]
fn warnings_still_go_to_stderr() {
    let mut resp = Response::ok("show", Some(json!({"tasks": []})));
    resp.warnings.push(moor_core::Warning {
        code: "w".into(),
        message: "careful".into(),
    });
    let (out, err) = render(&resp, false);
    assert_eq!(out, "no tasks\n");
    assert_eq!(err, "moor: warning: careful\n");
}

#[test]
fn detail_lists_proof_files() {
    let hash = "a".repeat(64);
    let out = show(json!({
        "task": task("x", true),
        "proofs": [{"name": "run.log", "size": 12, "sha256": hash}]
    }));
    assert!(
        out.contains(&format!("proof:\n  run.log  12  {hash}\n")),
        "{out}"
    );
}

fn with_attention(mut t: Value) -> Value {
    t.as_object_mut().unwrap().insert(
        "attention".into(),
        json!([{
            "kind": "agent_rate_limited", "rule": "claude", "status": 429,
            "message": "Claude account rate limited (429)"
        }]),
    );
    t
}

#[test]
fn list_prints_a_needs_attention_line_under_the_row() {
    let out = show(json!({"tasks": [with_attention(task("alpha", true)), task("b", true)]}));
    assert_eq!(
        out,
        "alpha  running  working  running  /work\n  needs attention: Claude account rate limited (429)\nb      running  working  running  /work\n"
    );
}

#[test]
fn detail_prints_a_needs_attention_line() {
    let out = show(json!({"task": with_attention(task("alpha", true))}));
    assert!(out.contains(
        "agent:    working pane=live\nneeds attention: Claude account rate limited (429)\n"
    ));
}

#[test]
fn detail_omits_the_line_without_attention() {
    let out = show(json!({"task": task("alpha", true)}));
    assert!(!out.contains("needs attention"));
}

#[test]
fn show_session_renders_host_with_child() {
    let v = json!({
        "session": 5, "box": null, "ordinal": 1, "parent": null, "workdir": "/w",
        "agent": "claude", "agent_session_id": "abc", "state": "working",
        "started_at": "t", "ended_at": null,
        "children": [{"session": 7, "box": "box-name", "state": "working"}]
    });
    let out = render(&Response::ok("show-session", Some(v)), false).0;
    assert_eq!(
        out,
        "Session 5 working (host) in /w\nagent session abc\n  child 7 box-name working\n"
    );
}
