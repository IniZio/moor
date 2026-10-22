//! Binary-level tests for the CLI contract: exit codes and the JSON envelope.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

#[path = "cli/client_cwd.rs"]
mod client_cwd;
#[path = "cli/controller.rs"]
mod controller;
#[path = "cli/events.rs"]
mod events;
#[path = "cli/fronts.rs"]
mod fronts;
#[path = "cli/inherited.rs"]
mod inherited;
#[path = "cli/redial_e2e.rs"]
mod redial_e2e;
#[path = "cli/run_output.rs"]
mod run_output;
#[path = "cli/ssh_controller.rs"]
mod ssh_controller;
#[path = "cli/user.rs"]
mod user;
#[path = "cli/via_controller.rs"]
mod via_controller;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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

/// A temp dir removed on drop that borrows as its path.
struct Dir(tempfile::TempDir);

impl std::ops::Deref for Dir {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<Path> for Dir {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<std::ffi::OsStr> for Dir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.path().as_os_str()
    }
}

/// Creates a fresh temp dir.
fn fresh_dir(tag: &str) -> Dir {
    Dir(tempfile::Builder::new()
        .prefix(&format!("moor-cli-{tag}-"))
        .tempdir()
        .unwrap())
}

/// Runs the binary hermetically with optional stdin.
fn run(args: &[&str], stdin: Option<&str>) -> Run {
    run_with_config(args, stdin, None)
}

/// Like `run`, with `config` written to the fresh HOME's default config path.
fn run_with_config(args: &[&str], stdin: Option<&str>, config: Option<&str>) -> Run {
    let home = fresh_dir("home");
    if let Some(text) = config {
        let dir = home.join(".config/moor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), text).unwrap();
    }
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

/// Not valid YAML for the config schema.
const BAD_CONFIG: &str = "tools: [unterminated\n  : : {";

#[test]
fn invalid_config_breaks_config_command() {
    let r = run_with_config(&["--json", "start", "foo"], None, Some(BAD_CONFIG));
    assert_ne!(r.code, 0);
    assert!(r.stdout.contains("config"), "stdout: {:?}", r.stdout);
}

#[test]
fn version_ignores_invalid_config() {
    let r = run_with_config(&["--json", "version"], None, Some(BAD_CONFIG));
    assert_eq!(r.code, 0, "stdout={:?}", r.stdout);
    let v: Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(v.get("class").and_then(Value::as_str), Some("ok"));
    let t = run_with_config(&["version"], None, Some(BAD_CONFIG));
    assert_eq!(t.code, 0);
}

#[test]
fn help_ignores_invalid_config() {
    for args in [&["help"][..], &["--help"][..]] {
        let r = run_with_config(args, None, Some(BAD_CONFIG));
        assert_eq!(r.code, 0, "{args:?}: {:?}", r.stderr);
    }
}

#[test]
fn json_pane_is_usage() {
    envelope(&["--json", "pane", "x"], None, 2, "usage");
}

#[test]
fn json_state_dir_equals_pane_is_usage() {
    let v = envelope(&["--state-dir=/tmp/x", "pane", "--json"], None, 2, "usage");
    assert_eq!(v.get("cmd").and_then(Value::as_str), Some("pane"));
}

#[test]
fn json_after_invalid_arg_is_usage() {
    envelope(&["start", "--bogus", "--json"], None, 2, "usage");
}

#[test]
fn json_no_subcommand_is_usage() {
    envelope(&["--json"], None, 2, "usage");
}

#[test]
fn json_start_outside_a_controller_is_usage() {
    envelope(&["--json", "start", "foo"], None, 2, "usage");
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
fn text_run_of_unknown_box_is_125_with_empty_stdout() {
    let r = run(&["run", "foo", "--", "true"], None);
    assert_eq!(r.code, 125, "stderr={:?}", r.stderr);
    assert!(r.stdout.is_empty(), "stdout: {:?}", r.stdout);
    assert!(!r.stderr.is_empty());
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

#[test]
fn controller_serve_without_config_is_usage() {
    let r = run(&["controller", "serve"], None);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--config"), "{}", r.stderr);
}
