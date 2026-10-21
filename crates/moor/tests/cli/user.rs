//! `moor user` and `moor whoami` act as the caller's `local:<uid>` through the controller.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use super::via_controller::Daemon;

const CANARY: &str = "ghp_canary_from_stdin_41d2";

/// Runs `moor --json <args>` against `daemon` with `input` on stdin; the envelope and the argv.
fn moor(daemon: &Daemon, args: &[&str], input: &str) -> (Value, Vec<String>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", daemon.root.join("client-home"))
        .env("MOOR_CONTROLLER", &daemon.socket)
        .arg("--json")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let spawned = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let envelope = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text:?}"));
    (envelope, spawned)
}

fn me() -> String {
    format!("local:{}", rustix::process::geteuid().as_raw())
}

#[test]
fn the_cli_enrolls_stores_a_secret_from_stdin_and_shows_whoami() {
    let daemon = Daemon::start();
    let (enrolled, _) = moor(&daemon, &["user", "enroll"], "");
    assert_eq!(enrolled["value"]["created"], true, "{enrolled}");
    let (set, argv) = moor(
        &daemon,
        &["user", "secret", "set", "github"],
        &format!("{CANARY}\n"),
    );
    assert_eq!(
        set["value"],
        json!({"name": "github", "changed": true}),
        "{set}"
    );
    assert!(argv.iter().all(|a| !a.contains(CANARY)), "{argv:?}");
    assert!(!set.to_string().contains(CANARY), "{set}");
    let (names, _) = moor(&daemon, &["user", "secret", "ls"], "");
    assert_eq!(names["value"]["names"], json!(["github"]), "{names}");
    let (who, _) = moor(&daemon, &["whoami"], "");
    assert_eq!(who["value"]["identity"], me(), "{who}");
    assert_eq!(who["value"]["identities"], json!([me()]), "{who}");
    let (gone, _) = moor(&daemon, &["user", "secret", "rm", "github"], "");
    assert_eq!(gone["value"]["changed"], true, "{gone}");
}

#[test]
fn the_cli_mints_a_link_code_and_refuses_it_for_an_already_linked_identity() {
    let daemon = Daemon::start();
    moor(&daemon, &["user", "enroll"], "");
    let (minted, _) = moor(&daemon, &["user", "link-code"], "");
    let code = minted["value"]["code"].as_str().unwrap().to_owned();
    let (linked, _) = moor(&daemon, &["user", "link", &code], "");
    assert_eq!(linked["class"], "refused", "{linked}");
    let message = linked["error"]["message"].as_str().unwrap();
    assert!(message.contains("already linked"), "{linked}");
}
