//! End-to-end tests for `moor events` against the local store.

use std::process::Command;

use serde_json::Value;

use super::fresh_dir;

fn events(args: &[&str]) -> std::process::Output {
    let root = fresh_dir("events");
    Command::new(env!("CARGO_BIN_EXE_moor"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("MOOR_STATE_DIR", root.join("state"))
        .arg("events")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn empty_store_prints_only_caught_up() {
    let out = events(&[]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains(r#""frame":"caught-up""#), "{text}");
}

#[test]
fn every_line_is_an_event_or_caught_up_frame() {
    let out = events(&["--since", "0", "--kind", "no-such-kind"]);
    assert_eq!(out.status.code(), Some(0));
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        let frame = v.get("frame");
        assert!(frame == Some(&Value::from("event")) || frame == Some(&Value::from("caught-up")));
    }
}
