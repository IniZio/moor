//! The record a question tool's hook writes, and the notice that follows it.

use moor_core::activity::{decide, AgentState, HookRead, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::seam::agent_profile::AgentProfile;
use moor_core::seam::clock::Ts;
use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{any_interpreter, ctx, forced, install, run_hook_on, state_file, Interp};

const QUESTION: &str = "Draft PR #32 already exists. How should I proceed?";
const LABELS: [&str; 3] = [
    "Reuse PR #32 (Recommended)",
    "Close #32, open new",
    "Use a different branch",
];

fn ask_payload() -> String {
    json!({
        "session_id": "s-42",
        "hook_event_name": "PreToolUse",
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{
            "question": QUESTION,
            "header": "Existing PR",
            "multiSelect": false,
            "options": LABELS.map(|l| json!({"label": l, "description": "d"})),
        }]},
    })
    .to_string()
}

fn notice_payload() -> String {
    json!({
        "session_id": "s-42",
        "message": "Claude needs your permission",
        "notification_type": "permission_prompt",
    })
    .to_string()
}

/// The state the hook file now gives, read as a probe would.
fn reported(raw: &[u8]) -> AgentState {
    let rec = Claude.parse_hook(raw).unwrap();
    let probes = Probes {
        hook: HookRead::Fresh(rec),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    };
    let now = Ts::from_millis(4_000_000_000_000);
    decide(&probes, None, now, Ts::from_millis(0), None).state
}

fn asking() -> AgentState {
    AgentState::NeedsInput {
        question: Some(QUESTION.to_owned()),
        options: LABELS.map(str::to_owned).to_vec(),
    }
}

fn check_question_and_options(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook_on(
        &c,
        "waiting_input",
        "PreToolUse",
        Some(&ask_payload()),
        path,
    );
    assert_eq!(reported(&state_file(&c)), asking());
}

fn check_notice_keeps_question(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook_on(
        &c,
        "waiting_input",
        "PreToolUse",
        Some(&ask_payload()),
        path,
    );
    run_hook_on(
        &c,
        "waiting_input",
        "Notification",
        Some(&notice_payload()),
        path,
    );
    assert_eq!(reported(&state_file(&c)), asking());
    run_hook_on(&c, "working", "PostToolUse", Some("{}"), path);
    run_hook_on(
        &c,
        "waiting_input",
        "Notification",
        Some(&notice_payload()),
        path,
    );
    let after = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(
        after.message.as_deref(),
        Some("Claude needs your permission")
    );
}

#[test]
fn a_question_tool_record_reports_its_question_and_options() {
    if any_interpreter() {
        check_question_and_options(None);
    }
}

#[test]
fn a_question_tool_record_reports_its_question_with_python3_only() {
    forced(Interp::Python, check_question_and_options);
}

#[test]
fn a_question_tool_record_reports_its_question_with_node_only() {
    forced(Interp::Node, check_question_and_options);
}

#[test]
fn a_permission_notice_leaves_a_pending_question_in_place() {
    if any_interpreter() {
        check_notice_keeps_question(None);
    }
}

#[test]
fn a_permission_notice_leaves_a_pending_question_in_place_with_node_only() {
    forced(Interp::Node, check_notice_keeps_question);
}
