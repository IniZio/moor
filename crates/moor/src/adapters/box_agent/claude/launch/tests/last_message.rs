//! The agent's last message, left in the note at the stop and dropped by the next prompt.

use moor_core::seam::agent_adapter::AgentAdapter;
use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, forced_with_core, install, note_of, on_host, run_hook_on, Interp};
use crate::testing::hc;

fn check_last_message_kept(core: &hc::Core, path: &str) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let stop = json!({"session_id": "s-1", "last_assistant_message": "Done.\nPR: https://x/1"});
    run_hook_on(&c, "Stop", Some(&stop.to_string()), Some(path));
    let note = note_of(core);
    assert_eq!(note.said.as_deref(), Some("Done.\nPR: https://x/1"));
    assert_eq!(note.session_id.as_deref(), Some("s-1"));
    run_hook_on(
        &c,
        "UserPromptSubmit",
        Some(r#"{"session_id":"s-1"}"#),
        Some(path),
    );
    let note = note_of(core);
    assert_eq!(note.said, None);
    assert_eq!(note.session_id.as_deref(), Some("s-1"));
}

#[test]
fn the_last_message_is_dropped_by_the_next_prompt() {
    on_host(check_last_message_kept);
}

#[test]
fn the_last_message_is_dropped_by_the_next_prompt_with_python3_only() {
    forced_with_core(Interp::Python, check_last_message_kept);
}

#[test]
fn the_last_message_is_dropped_by_the_next_prompt_with_node_only() {
    forced_with_core(Interp::Node, check_last_message_kept);
}

/// A reply Claude Code wrote as several text blocks split by thinking, after a tool call, where
/// the Stop payload's `last_assistant_message` holds only the final block.
fn check_split_reply_kept_whole(core: &hc::Core, path: &str) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let transcript = t.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        include_str!("../../fixtures/split_reply.jsonl"),
    )
    .unwrap();
    let stop = json!({
        "session_id": "s-1",
        "transcript_path": transcript.to_str().unwrap(),
        "last_assistant_message": "**`free -m`:** 7937 total.",
    });
    run_hook_on(&c, "Stop", Some(&stop.to_string()), Some(path));
    assert_eq!(
        note_of(core).said.as_deref(),
        Some("Summary: the disk is full; /home is at 100%.\n\n**`free -m`:** 7937 total.")
    );
}

/// A transcript that has not caught up with the Stop payload is not trusted over it.
fn check_stale_transcript_ignored(core: &hc::Core, path: &str) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let transcript = t.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        include_str!("../../fixtures/split_reply.jsonl"),
    )
    .unwrap();
    let stop = json!({
        "transcript_path": transcript.to_str().unwrap(),
        "last_assistant_message": "A newer reply.",
    });
    run_hook_on(&c, "Stop", Some(&stop.to_string()), Some(path));
    assert_eq!(note_of(core).said.as_deref(), Some("A newer reply."));
}

#[test]
fn a_reply_split_into_blocks_is_kept_whole_with_python3_only() {
    forced_with_core(Interp::Python, check_split_reply_kept_whole);
    forced_with_core(Interp::Python, check_stale_transcript_ignored);
}

#[test]
fn a_reply_split_into_blocks_is_kept_whole_with_node_only() {
    forced_with_core(Interp::Node, check_split_reply_kept_whole);
    forced_with_core(Interp::Node, check_stale_transcript_ignored);
}
