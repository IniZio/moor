//! The plugin's pane entry and the config snippet as a user installs them.

#![allow(clippy::unwrap_used, reason = "a test fails on any setup error")]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const RUN_SH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/herdr/bin/run.sh");

fn setup_shell(env: &[(&str, &Path)]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor-herdr"));
    cmd.arg("herdr-setup")
        .env_remove("HERDR_PLUGIN_STATE_DIR")
        .env_remove("HERDR_PLUGIN_ROOT");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("default_shell = "))
        .unwrap();
    line.trim_start_matches("default_shell = ")
        .trim_matches('"')
        .to_owned()
}

#[test]
fn herdr_setup_names_the_installed_shell_under_xdg_state_home() {
    let xdg = tempfile::tempdir().unwrap();
    assert_eq!(
        setup_shell(&[("XDG_STATE_HOME", xdg.path())]),
        format!("{}/herdr/plugins/moor/shell.sh", xdg.path().display())
    );
}

#[test]
fn herdr_setup_names_the_shell_in_herdrs_plugin_state_dir() {
    let xdg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    assert_eq!(
        setup_shell(&[
            ("XDG_STATE_HOME", xdg.path()),
            ("HERDR_PLUGIN_STATE_DIR", state.path())
        ]),
        format!("{}/shell.sh", state.path().display())
    );
}

fn run_sh(state: &Path, bin: &Path, args: &[&str]) -> Child {
    Command::new("sh")
        .arg(RUN_SH)
        .args(args)
        .env("HERDR_PLUGIN_STATE_DIR", state)
        .env("MOOR_HERDR_BIN", bin)
        .env_remove("HERDR_PLUGIN_CONFIG_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn failing_stub(dir: &Path) -> std::path::PathBuf {
    let stub = dir.join("moor-herdr");
    std::fs::write(
        &stub,
        "#!/bin/sh\necho 'moor-herdr: not signed in to https://moor.example.com' >&2\nexit 2\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    stub
}

/// Reads the pane's screen up to its prompt, checks it is still waiting, then presses a key.
fn held_screen(mut child: Child) -> (String, Option<i32>) {
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut screen = String::new();
    while !screen.contains("press any key to close") {
        assert_ne!(
            out.read_line(&mut screen).unwrap(),
            0,
            "closed early: {screen}"
        );
    }
    thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "exited before a key");
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    let status = child.wait().unwrap();
    out.read_to_string(&mut screen).unwrap();
    (screen, status.code())
}

#[test]
fn a_pane_that_fails_keeps_its_error_on_screen_until_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let child = run_sh(dir.path(), &failing_stub(dir.path()), &["panel"]);
    let (screen, code) = held_screen(child);
    assert!(screen.contains("moor: panel failed (exit 2)"), "{screen}");
    assert!(
        screen.contains("not signed in to https://moor.example.com"),
        "{screen}"
    );
    assert_eq!(code, Some(2));
}

#[test]
fn a_pane_with_no_binary_says_so_until_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let child = run_sh(dir.path(), &dir.path().join("absent"), &["new"]);
    let (screen, code) = held_screen(child);
    assert!(screen.contains("moor: new failed (exit 127)"), "{screen}");
    assert!(screen.contains("no moor-herdr at"), "{screen}");
    assert_eq!(code, Some(127));
}

#[test]
fn an_action_that_fails_exits_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let child = run_sh(dir.path(), &failing_stub(dir.path()), &["sync"]);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not signed in"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("press any key"));
}
