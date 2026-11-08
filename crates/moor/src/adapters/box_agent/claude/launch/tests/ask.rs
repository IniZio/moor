//! The note a question tool's hook leaves, and the state it gives once the agent blocks.

use moor_core::activity::{decide, AgentState, BlockKind, Status};
use moor_core::seam::agent_adapter::AgentAdapter;
use moor_core::seam::clock::Ts;
use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced_with_core, install, note_of, on_host, run_hook_on, Interp};
use crate::testing::agent;
use crate::testing::hc;

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

/// The state a blocked-on-a-question agent with the note now in `core` reads as.
fn reported(core: &hc::Core) -> AgentState {
    let at = Ts::from_millis(4_000_000_000_000);
    let blocked = agent::read(
        Some(Status::Blocked {
            kind: Some(BlockKind::Question),
            msg: None,
        }),
        at,
        1,
    );
    let probe = agent::with_note(blocked, note_of(core));
    decide(&probe, at, Ts::from_millis(0), None).state
}

fn asking() -> AgentState {
    AgentState::NeedsInput {
        question: Some(QUESTION.to_owned()),
        options: LABELS.map(str::to_owned).to_vec(),
    }
}

fn check_question_and_options(core: &hc::Core, path: &str) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook_on(&c, "PreToolUse", Some(&ask_payload()), Some(path));
    assert_eq!(reported(core), asking());
}

#[test]
fn a_question_tool_note_reports_its_question_and_options() {
    on_host(check_question_and_options);
}

#[test]
fn a_question_tool_note_reports_its_question_with_python3_only() {
    forced_with_core(Interp::Python, check_question_and_options);
}

#[test]
fn a_question_tool_note_reports_its_question_with_node_only() {
    forced_with_core(Interp::Node, check_question_and_options);
}
