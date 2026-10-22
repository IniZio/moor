//! `controller: {ssh: <dest>}` drives a real controller through a fake `ssh`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::fresh_dir;
use super::via_controller::Daemon;

/// Writes an executable script at `dir/ssh`.
fn fake_ssh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("ssh");
    moor_testkit::write_executable(&path, format!("#!/bin/sh\n{body}")).unwrap();
    path
}

/// Runs `moor --json show` with the user config pointing at `ssh: tester@remotehost`.
fn run_show(dir: &Path, ssh: &Path) -> (i32, String) {
    let config = dir.join("client.yaml");
    std::fs::write(&config, "controller:\n  ssh: tester@remotehost\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_moor"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir.join("client-home"))
        .env("MOOR_CONFIG", &config)
        .env("MOOR_SSH_BIN", ssh)
        .env("MOOR_CONTROLLER_REDIAL_SECS", "0")
        .args(["--json", "show"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap()
    );
    (out.status.code().unwrap(), text)
}

#[test]
fn ssh_controller_runs_attach_and_serves_the_verb() {
    let daemon = Daemon::start();
    let argv = daemon.root.join("argv.txt");
    let ssh = fake_ssh(
        &daemon.root,
        &format!(
            "printf '%s\\n' \"$*\" > {argv}\n\
             [ \"$1\" = -T ] && [ \"$2\" = tester@remotehost ] || exit 97\n\
             exec {moor} controller attach --socket {sock}\n",
            argv = argv.display(),
            moor = env!("CARGO_BIN_EXE_moor"),
            sock = daemon.socket.display(),
        ),
    );
    let (code, text) = run_show(&daemon.root, &ssh);
    assert_eq!(code, 0, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text:?}"));
    assert_eq!(v.get("class").and_then(Value::as_str), Some("ok"), "{text}");
    let recorded = std::fs::read_to_string(&argv).unwrap();
    assert_eq!(
        recorded.trim_end(),
        "-T tester@remotehost moor controller attach"
    );
}

#[test]
fn ssh_failure_exits_one_and_names_the_host() {
    let dir = fresh_dir("ssh-down");
    let ssh = fake_ssh(
        &dir,
        "echo 'ssh: connect to host remotehost port 22: Connection refused' >&2\nexit 255\n",
    );
    let (code, text) = run_show(&dir, &ssh);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("remotehost"), "{text}");
    let _ = std::fs::remove_dir_all(dir);
}
