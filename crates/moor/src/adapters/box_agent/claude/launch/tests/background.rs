//! The hook script keeps the background work still running at a stop.

use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced_with_core, install, note_of, on_host, run_hook_on, Interp};
use crate::testing::hc;
use moor_core::seam::agent_adapter::AgentAdapter;

fn check_only_running_background_work_is_kept(core: &hc::Core, path: &str) {
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
    run_hook_on(&c, "Stop", Some(&stop.to_string()), Some(path));
    let note = note_of(core);
    assert_eq!(note.background, ["gh run watch", "agent"]);
    assert_eq!(note.said.as_deref(), Some("Watching the run."));
    let done = json!({"last_assistant_message": "Done.", "background_tasks": []});
    run_hook_on(&c, "Stop", Some(&done.to_string()), Some(path));
    assert!(note_of(core).background.is_empty());
}

#[test]
fn only_running_background_work_is_kept() {
    on_host(check_only_running_background_work_is_kept);
}

#[test]
fn only_running_background_work_is_kept_with_python3_only() {
    forced_with_core(Interp::Python, check_only_running_background_work_is_kept);
}

#[test]
fn only_running_background_work_is_kept_with_node_only() {
    forced_with_core(Interp::Node, check_only_running_background_work_is_kept);
}
