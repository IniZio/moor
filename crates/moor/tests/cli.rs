//! Binary-level tests for the CLI contract: exit codes and the JSON envelope.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

/// Captured result of one binary run.
struct Run {
    /// Exit code.
    code: i32,
    /// Captured stdout.
    stdout: String,
    /// Captured stderr.
    stderr: String,
}

/// Counter making temp dir names unique within this process.
static SEQ: AtomicU32 = AtomicU32::new(0);

/// Creates a fresh temp dir.
fn fresh_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("moor-cli-{}-{tag}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the binary hermetically with optional stdin.
fn run(args: &[&str], stdin: Option<&str>) -> Run {
    let home = fresh_dir("home");
    let state = fresh_dir("state");
    let mut child = Command::new(env!("CARGO_BIN_EXE_moor"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("MOOR_STATE_DIR", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(s) = stdin {
        pipe.write_all(s.as_bytes()).unwrap();
    }
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(state);
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// Asserts exit code, empty stderr, and one envelope; returns it.
fn envelope(args: &[&str], stdin: Option<&str>, code: i32, class: &str) -> Value {
    let r = run(args, stdin);
    assert_eq!(r.code, code, "exit code; stdout={:?}", r.stdout);
    assert!(r.stderr.is_empty(), "stderr not empty: {:?}", r.stderr);
    let items: Vec<Value> = serde_json::Deserializer::from_str(&r.stdout)
        .into_iter::<Value>()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(items.len(), 1, "expected one JSON value: {:?}", r.stdout);
    let v = items.into_iter().next().unwrap();
    let obj = v.as_object().expect("envelope is an object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["class", "cmd", "error", "schema", "value", "warnings"]
    );
    assert_eq!(obj.get("class").and_then(Value::as_str), Some(class));
    v
}

#[test]
fn version_text() {
    let r = run(&["version"], None);
    assert_eq!(r.code, 0);
    assert!(!r.stdout.is_empty());
    assert!(r.stderr.is_empty(), "stderr: {:?}", r.stderr);
}

#[test]
fn json_version() {
    envelope(&["--json", "version"], None, 0, "ok");
}

#[test]
fn json_pane_is_usage() {
    envelope(&["--json", "pane", "x"], None, 2, "usage");
}

#[test]
fn json_no_subcommand_is_usage() {
    envelope(&["--json"], None, 2, "usage");
}

#[test]
fn json_start_is_failed() {
    envelope(&["--json", "start", "foo"], None, 1, "failed");
}

#[test]
fn json_start_bad_name_is_usage() {
    envelope(&["--json", "start", "BAD_Name"], None, 2, "usage");
}

#[test]
fn json_run_is_125() {
    envelope(&["--json", "run", "foo", "--", "true"], None, 125, "failed");
}

#[test]
fn json_run_without_argv_is_125() {
    envelope(&["--json", "run", "foo"], None, 125, "usage");
}

#[test]
fn call_unknown_field_is_usage() {
    envelope(
        &["call"],
        Some(r#"{"cmd":"version","bogus":1}"#),
        2,
        "usage",
    );
}

#[test]
fn json_help_is_one_envelope() {
    let v = envelope(&["--json", "--help"], None, 0, "ok");
    assert!(v.get("value").is_some_and(Value::is_string));
}
