//! The hook script keeps the background work still running at a stop.

use moor_core::activity::HookKind;
use moor_core::seam::agent_adapter::AgentAdapter;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced, install, run_hook_on, state_file, Interp};

fn check_only_running_background_work_is_kept(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let stop = json!({
        "session_id": "s-1",
        "last_assistant_message": "Watching the run.",
        "background_tasks": [
            {"id": "a1", "type": "shell", "status": "running", "description": "gh run watch"},
            {"id": "b2", "type": "shell", "status": "completed", "description": "old build"},
            {"id": "c3", "type": "agent"}
        ],
        "session_crons": [{"id": "x", "schedule": "* * * * *"}]
    });
    run_hook_on(&c, "idle", "Stop", Some(&stop.to_string()), path);
    let raw = state_file(&c);
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(v["background"], json!(["gh run watch", "agent"]));
    let rec = Claude.parse_hook(&raw).unwrap();
    assert_eq!(
        rec.kind,
        HookKind::Waiting {
            on: vec!["gh run watch".to_owned(), "agent".to_owned()]
        }
    );
    let nudge = json!({"notification_type": "idle_prompt", "message": "waiting"});
    run_hook_on(
        &c,
        "waiting_input",
        "Notification",
        Some(&nudge.to_string()),
        path,
    );
    assert!(matches!(
        Claude.parse_hook(&state_file(&c)).unwrap().kind,
        HookKind::Waiting { .. }
    ));
    let done = json!({"last_assistant_message": "Done.", "background_tasks": []});
    run_hook_on(&c, "idle", "Stop", Some(&done.to_string()), path);
    assert_eq!(
        Claude.parse_hook(&state_file(&c)).unwrap().kind,
        HookKind::Idle
    );
}

#[test]
fn only_running_background_work_is_kept_with_python3_only() {
    forced(Interp::Python, check_only_running_background_work_is_kept);
}

#[test]
fn only_running_background_work_is_kept_with_node_only() {
    forced(Interp::Node, check_only_running_background_work_is_kept);
}
