//! The agent's last message, kept in the hook record from the stop until the next prompt.

use moor_core::activity::HookKind;
use moor_core::seam::agent_adapter::AgentAdapter;
use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced, install, run_hook_on, state_file, Interp};

fn check_last_message_kept(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let stop = json!({"session_id": "s-1", "last_assistant_message": "Done.\nPR: https://x/1"});
    run_hook_on(&c, "idle", "Stop", Some(&stop.to_string()), path);
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.kind, HookKind::Idle);
    assert_eq!(rec.message.as_deref(), Some("Done.\nPR: https://x/1"));
    let nudge = json!({"notification_type": "idle_prompt", "message": "waiting"});
    run_hook_on(
        &c,
        "waiting_input",
        "Notification",
        Some(&nudge.to_string()),
        path,
    );
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.message.as_deref(), Some("Done.\nPR: https://x/1"));
    run_hook_on(&c, "working", "UserPromptSubmit", Some("{}"), path);
    run_hook_on(
        &c,
        "waiting_input",
        "Notification",
        Some(&nudge.to_string()),
        path,
    );
    let rec = Claude.parse_hook(&state_file(&c)).unwrap();
    assert_eq!(rec.message, None);
}

#[test]
fn the_last_message_survives_the_idle_nudge_with_python3_only() {
    forced(Interp::Python, check_last_message_kept);
}

#[test]
fn the_last_message_survives_the_idle_nudge_with_node_only() {
    forced(Interp::Node, check_last_message_kept);
}
