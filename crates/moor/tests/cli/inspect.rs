//! `moor inspect` through the controller socket, and its argument errors.

use std::io::Write;
use std::process::{Command, Stdio};

use moor_testkit::ChildGuard;
use serde_json::Value;

use super::via_controller::Daemon;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn moor(daemon: &Daemon, socket: bool, args: &[&str], stdin: &str) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", daemon.root.join("client-home"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if socket {
        cmd.env("MOOR_CONTROLLER", &daemon.socket);
    } else {
        cmd.env("MOOR_CONTROLLER", daemon.root.join("nothing-listens.sock"));
    }
    let mut child = ChildGuard::new(cmd.spawn().unwrap());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    Out {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

#[test]
fn call_inspect_of_a_missing_box_answers_an_error_envelope() {
    let daemon = Daemon::start();
    let out = moor(
        &daemon,
        true,
        &["call"],
        r#"{"cmd":"inspect","box":"app/nope"}"#,
    );
    let envelope: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_ne!(envelope.get("class").and_then(Value::as_str), Some("ok"));
    assert!(out.stdout.contains("no Box"), "{}", out.stdout);
    assert_ne!(out.code, 0);
}

#[test]
fn inspect_of_a_missing_box_fails_with_the_message() {
    let daemon = Daemon::start();
    let out = moor(&daemon, true, &["inspect", "app/nope"], "");
    assert_ne!(out.code, 0, "{}", out.stdout);
    let said = format!("{}{}", out.stdout, out.stderr);
    assert!(said.contains("no Box"), "{said}");
}

#[test]
fn a_bad_line_count_or_a_missing_name_fails_parsing_without_the_controller() {
    let daemon = Daemon::start();
    for args in [
        &["inspect", "app/x", "--lines", "many"][..],
        &["inspect"][..],
    ] {
        let out = moor(&daemon, false, args, "");
        assert_eq!(out.code, 2, "{args:?}: {}{}", out.stdout, out.stderr);
    }
}
