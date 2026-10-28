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

/// `user`'s secret `name` as the controller stored it, opened with the vault key beside its
/// config; the sealed value starts with a `<user> <name>` line the vault checks.
fn stored_secret(daemon: &Daemon, user: i64, name: &str) -> Vec<u8> {
    let db = rusqlite::Connection::open(daemon.root.join("state").join("moor.db")).unwrap();
    let sealed: Vec<u8> = db
        .query_row(
            "SELECT value FROM user_secrets WHERE user = ? AND name = ?",
            rusqlite::params![user, name],
            |r| r.get(0),
        )
        .unwrap();
    let key = std::fs::read_to_string(daemon.root.join("vault.key")).unwrap();
    let identity: age::x25519::Identity = key.trim().parse().unwrap();
    let bound = age::decrypt(&identity, &sealed).unwrap();
    bound
        .strip_prefix(format!("{user} {name}\n").as_bytes())
        .unwrap()
        .to_vec()
}

#[test]
fn the_cli_stores_exactly_the_stdin_value() {
    let daemon = Daemon::start();
    let (enrolled, _) = moor(&daemon, &["user", "enroll"], "");
    let user = enrolled["value"]["user"].as_i64().unwrap();
    let (set, _) = moor(&daemon, &["user", "secret", "set", "github"], CANARY);
    assert_eq!(set["value"]["changed"], true, "{set}");
    assert_eq!(stored_secret(&daemon, user, "github"), CANARY.as_bytes());
}
