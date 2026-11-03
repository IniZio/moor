//! The status the hook script prints to the agent's terminal.

use std::fs;
use std::process::{Command, Stdio};

use tempfile::TempDir;

use super::super::super::Claude;
use super::{ctx, install, run_hook_on};
use moor_core::seam::agent_adapter::AgentAdapter;

fn osc(state: &str) -> String {
    format!("\x1b]7501;state={state}:app=claude\x1b\\")
}

/// Runs the hook for each `(state, event)` in turn with `MOOR_AGENT_TTY` set to `tty`, and
/// returns what landed in the file.
fn printed(events: &[(&str, &str)], tty: Option<&str>, tmux: bool) -> Vec<u8> {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let device = t.path().join("tty");
    fs::write(&device, b"").unwrap();
    let script = format!("{}/.moor/hook.sh", c.workdir.as_str());
    let device = device.to_str().unwrap().to_owned();
    for (state, event) in events {
        let mut cmd = Command::new("/bin/sh");
        cmd.args([script.as_str(), state, event])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .env_remove("MOOR_AGENT_TTY")
            .env_remove("TMUX");
        match tty {
            Some("") => cmd.env("MOOR_AGENT_TTY", ""),
            Some(_) | None => cmd.env("MOOR_AGENT_TTY", &device),
        };
        if tmux {
            cmd.env("TMUX", "/tmp/tmux-0/default,1,0");
        }
        assert!(cmd.status().unwrap().success());
    }
    fs::read(&device).unwrap()
}

#[test]
fn each_mapped_event_prints_its_state_to_the_device() {
    for (state, event, want) in [
        ("working", "UserPromptSubmit", "working"),
        ("working", "PostToolUse", "working"),
        ("waiting_input", "Notification", "blocked:kind=permission"),
        ("waiting_input", "PreToolUse", "blocked:kind=question"),
        ("idle", "Stop", "idle"),
        ("idle", "SessionStart", "idle"),
        ("done", "SessionEnd", "done"),
    ] {
        let out = printed(&[(state, event)], None, false);
        assert_eq!(out, osc(want).into_bytes(), "{state} {event}");
    }
}

#[test]
fn nothing_is_printed_without_a_device() {
    let out = printed(&[("working", "UserPromptSubmit")], Some(""), false);
    assert!(out.is_empty());
}

#[test]
fn the_same_state_twice_prints_once() {
    let out = printed(
        &[
            ("working", "PostToolUse"),
            ("working", "PostToolUse"),
            ("idle", "Stop"),
            ("idle", "Stop"),
            ("working", "UserPromptSubmit"),
        ],
        None,
        false,
    );
    let want = [osc("working"), osc("idle"), osc("working")].concat();
    assert_eq!(out, want.into_bytes());
}

#[test]
fn a_session_end_leaves_the_state_record_alone() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    run_hook_on(&c, "done", "SessionEnd", None, None);
    let record = format!("{}/.moor/agent-state.json", c.workdir.as_str());
    assert!(!std::path::Path::new(&record).exists());
}

#[test]
fn inside_tmux_the_sequence_travels_in_a_passthrough_wrapper() {
    let out = printed(&[("working", "UserPromptSubmit")], None, true);
    let want = "\x1bPtmux;\x1b\x1b]7501;state=working:app=claude\x1b\x1b\\\x1b\\";
    assert_eq!(out, want.as_bytes());
}
