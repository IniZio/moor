//! The `doing` line a tool-use hook leaves in the record, and what it never carries.

use moor_core::activity::HookKind;
use moor_core::seam::agent_adapter::AgentAdapter;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced, install, run_hook_on, state_file, Interp};

fn tool(name: &str, input: &Value) -> String {
    json!({
        "session_id": "s-1",
        "tool_name": name,
        "tool_input": input,
        "tool_response": {"stdout": "TOPSECRET-OUTPUT"},
    })
    .to_string()
}

/// The line `event` leaves for `payload`, and the raw state file.
fn left(event: &str, payload: &str, path: Option<&str>) -> (Option<String>, String) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook_on(&c, "working", event, Some(payload), path);
    let raw = state_file(&c);
    let rec = Claude.parse_hook(&raw).unwrap();
    assert_eq!(rec.kind, HookKind::Busy);
    (rec.doing, String::from_utf8(raw).unwrap())
}

fn check_lines(path: Option<&str>) {
    let doing = |name: &str, input: Value| left("PostToolUse", &tool(name, &input), path).0;
    assert_eq!(
        doing("Bash", json!({"command": "cargo test -p moor --locked"})).as_deref(),
        Some("Bash: cargo test")
    );
    assert_eq!(
        doing("Bash", json!({"command": "RUST_LOG=debug ./run.sh --all"})).as_deref(),
        Some("Bash: run.sh")
    );
    assert_eq!(
        doing(
            "Bash",
            json!({"command": "cd backend && docker compose up -d"})
        )
        .as_deref(),
        Some("Bash: docker compose")
    );
    assert_eq!(
        doing("Bash", json!({"command": "cd /tmp; ls -la"})).as_deref(),
        Some("Bash: ls")
    );
    assert_eq!(
        doing("Bash", json!({"command": "cd /tmp"})).as_deref(),
        Some("Bash: cd")
    );
    assert_eq!(
        doing("Edit", json!({"file_path": "/work/src/main.rs"})).as_deref(),
        Some("Edit: main.rs")
    );
    assert_eq!(
        doing(
            "WebFetch",
            json!({"url": "https://u:pw@example.com:8443/a?token=1"})
        )
        .as_deref(),
        Some("WebFetch: example.com:8443")
    );
    assert_eq!(
        doing("Grep", json!({"pattern": "AKIA[0-9A-Z]{16}"})).as_deref(),
        Some("Grep")
    );
}

fn check_secrets_stay_out(path: Option<&str>) {
    let leaky = tool(
        "Bash",
        &json!({"command": "curl -H 'Authorization: Bearer sk-ant-ZZZ' https://x.test"}),
    );
    let (doing, raw) = left("PreToolUse", &leaky, path);
    assert_eq!(doing.as_deref(), Some("Bash: curl"));
    assert!(!raw.contains("TOPSECRET-OUTPUT"), "{raw}");
    let echo = tool("Bash", &json!({"command": "echo sk-ant-api03-AbC123"}));
    assert_eq!(
        left("PostToolUse", &echo, path).0.as_deref(),
        Some("Bash: echo")
    );
}

fn check_idle_drops_it(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let payload = tool("Bash", &json!({"command": "make"}));
    run_hook_on(&c, "working", "PostToolUse", Some(&payload), path);
    run_hook_on(&c, "idle", "Stop", Some("{}"), path);
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.kind, HookKind::Idle);
    assert_eq!(rec.doing, None);
}

#[test]
fn a_tool_use_names_what_is_running_with_python3_only() {
    forced(Interp::Python, check_lines);
}

#[test]
fn a_tool_use_names_what_is_running_with_node_only() {
    forced(Interp::Node, check_lines);
}

#[test]
fn no_output_or_secret_shaped_word_reaches_the_line_with_python3_only() {
    forced(Interp::Python, check_secrets_stay_out);
}

#[test]
fn no_output_or_secret_shaped_word_reaches_the_line_with_node_only() {
    forced(Interp::Node, check_secrets_stay_out);
}

#[test]
fn an_idle_record_carries_no_line_with_python3_only() {
    forced(Interp::Python, check_idle_drops_it);
}

#[test]
fn an_idle_record_carries_no_line_with_node_only() {
    forced(Interp::Node, check_idle_drops_it);
}

#[test]
fn every_tool_but_the_question_tools_reports_busy_before_it_runs() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let setup = Claude.hook_writer(&c).unwrap();
    let fragment = setup
        .files
        .iter()
        .find(|f| f.path.as_str().ends_with("hooks.json"))
        .unwrap();
    let frag: Value = serde_json::from_str(&fragment.content).unwrap();
    let busy: Vec<&Value> = frag["hooks"]["PreToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(" working ")
        })
        .collect();
    assert_eq!(busy.len(), 1);
    let matcher = busy[0]["matcher"].as_str().unwrap();
    for name in ["Bash", "Edit", "WebFetch"] {
        assert!(matcher.split('|').any(|m| m == name), "{name} in {matcher}");
    }
    for name in ["AskUserQuestion", "ExitPlanMode"] {
        assert!(
            !matcher.split('|').any(|m| m == name),
            "{name} in {matcher}"
        );
    }
}

fn check_permission_after_busy(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let payload = tool("Bash", &json!({"command": "rm -rf build"}));
    run_hook_on(&c, "working", "PreToolUse", Some(&payload), path);
    let notice = json!({"message": "Allow rm?", "notification_type": "permission_prompt"});
    run_hook_on(
        &c,
        "waiting_input",
        "Notification",
        Some(&notice.to_string()),
        path,
    );
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.kind, HookKind::Blocked);
}

#[test]
fn a_permission_notice_after_a_busy_tool_still_reports_waiting_with_python3_only() {
    forced(Interp::Python, check_permission_after_busy);
}

#[test]
fn a_permission_notice_after_a_busy_tool_still_reports_waiting_with_node_only() {
    forced(Interp::Node, check_permission_after_busy);
}
