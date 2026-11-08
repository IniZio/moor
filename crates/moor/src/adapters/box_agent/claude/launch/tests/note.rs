//! What the hook leaves in herdr-core's `moor-agent` doc.

use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, install, run_hook_on};
use crate::testing::hc;
use moor_core::seam::agent_adapter::{AgentAdapter, LaunchCtx};

/// A hook installed for a fresh workdir.
fn hooked() -> (TempDir, LaunchCtx) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    (t, c)
}

fn fire(core: &hc::Core, c: &LaunchCtx, event: &str, input: &Value) {
    run_hook_on(c, event, Some(&input.to_string()), Some(&core.path()));
}

#[test]
fn a_stop_leaves_the_last_message_and_session() {
    let Some(core) = hc::core() else { return };
    let (_t, c) = hooked();
    fire(
        &core,
        &c,
        "Stop",
        &json!({"session_id": "s1", "last_assistant_message": "All done."}),
    );
    assert_eq!(
        core.doc("moor-agent"),
        json!({"session_id": "s1", "said": "All done."})
    );
}

#[test]
fn a_question_leaves_its_questions_and_a_prompt_clears_them() {
    let Some(core) = hc::core() else { return };
    let (_t, c) = hooked();
    let questions =
        json!([{"question": "Which fruit?", "options": [{"label": "Apple"}, {"label": "Banana"}]}]);
    fire(
        &core,
        &c,
        "PreToolUse",
        &json!({"session_id": "s1", "tool_name": "AskUserQuestion", "tool_input": {"questions": questions}}),
    );
    assert_eq!(core.doc("moor-agent")["questions"], questions);
    fire(&core, &c, "UserPromptSubmit", &json!({"session_id": "s1"}));
    assert_eq!(core.doc("moor-agent"), json!({"session_id": "s1"}));
}

#[test]
fn a_box_without_herdr_core_still_runs_the_hook() {
    let (_t, c) = hooked();
    run_hook_on(
        &c,
        "Stop",
        Some(r#"{"session_id":"s1"}"#),
        Some("/usr/bin:/bin"),
    );
}
