#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::disallowed_methods,
    missing_docs,
    unreachable_pub,
    reason = "tests assert on fixed fixtures"
)]

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use rig::{Rig, HEADERS};

mod rig;

fn ino(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn a_request_missing_a_header_is_refused_naming_it() {
    let rig = Rig::start();
    for (missing, _) in HEADERS {
        let headers: Vec<_> = HEADERS.into_iter().filter(|(k, _)| *k != missing).collect();
        let err = rig
            .call_with("exec", json!({"command": "true"}), &headers)
            .unwrap_err();
        assert!(err.ends_with(&format!("missing header {missing}")), "{err}");
    }
}

#[test]
fn sync_copies_the_tree_and_a_second_sync_sends_only_the_change() {
    let rig = Rig::start();
    rig.write("a.txt", "one");
    rig.write("sub/b.txt", "two");
    rig.write(".git/HEAD", "ref");
    rig.write("node_modules/m/index.js", "native");
    rig.call("sync", json!({})).unwrap();
    let copy = rig.copy();
    assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "one");
    assert_eq!(
        std::fs::read_to_string(copy.join("sub/b.txt")).unwrap(),
        "two"
    );
    assert!(
        copy.join("node_modules/m/index.js").exists(),
        "node_modules is kept"
    );
    assert!(!copy.join(".git").exists(), ".git is left out");
    let (a, b) = (ino(&copy.join("a.txt")), ino(&copy.join("sub/b.txt")));
    rig.write("a.txt", "one, changed");
    rig.call("sync", json!({})).unwrap();
    let changed = std::fs::read_to_string(copy.join("a.txt")).unwrap();
    assert_eq!(changed, "one, changed");
    assert_ne!(ino(&copy.join("a.txt")), a, "the changed file was sent");
    assert_eq!(
        ino(&copy.join("sub/b.txt")),
        b,
        "the unchanged file was not sent again"
    );
}

#[test]
fn sync_removes_a_file_deleted_in_the_box() {
    let rig = Rig::start();
    rig.write("keep.txt", "k");
    rig.write("gone.txt", "g");
    rig.call("sync", json!({})).unwrap();
    assert!(rig.copy().join("gone.txt").exists());
    std::fs::remove_file(rig.tree.join("gone.txt")).unwrap();
    rig.call("sync", json!({})).unwrap();
    assert!(
        !rig.copy().join("gone.txt").exists(),
        "deleted on the machine too"
    );
    assert!(rig.copy().join("keep.txt").exists());
}

#[test]
fn exec_runs_in_the_synced_copy_and_returns_code_and_output() {
    let rig = Rig::start();
    rig.write("a.txt", "hello");
    rig.call("sync", json!({})).unwrap();
    let command = "pwd; cat a.txt; echo oops >&2; exit 3";
    let out = rig.call("exec", json!({ "command": command })).unwrap();
    assert_eq!(out["exit_code"], 3, "{out}");
    assert_eq!(out["timed_out"], false);
    let output = out["output"].as_str().unwrap();
    assert!(
        output.contains(&rig.copy().display().to_string()),
        "{output}"
    );
    assert!(
        output.contains("hello") && output.contains("oops"),
        "{output}"
    );
    assert_eq!(out["truncated"], false);
}

#[test]
fn exec_output_over_64_kib_is_cut_to_its_tail() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let command = "head -c 70000 /dev/zero | tr '\\0' a; printf END";
    let out = rig.call("exec", json!({ "command": command })).unwrap();
    let output = out["output"].as_str().unwrap();
    assert_eq!(output.len(), 64 * 1024);
    assert!(output.ends_with("aEND"), "the tail is kept");
    assert_eq!(out["truncated"], true);
    assert_eq!(out["exit_code"], 0);
}

#[test]
fn exec_past_its_timeout_is_killed_and_says_so() {
    let rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let err = rig
        .call("exec", json!({"command": "true", "timeout_secs": 3601}))
        .unwrap_err();
    assert!(err.contains("3600"), "{err}");
    let started = Instant::now();
    let command = "echo started; sleep 30";
    let out = rig
        .call("exec", json!({"command": command, "timeout_secs": 1}))
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "returned at the timeout"
    );
    assert_eq!(out["timed_out"], true, "{out}");
    assert_eq!(out["exit_code"], json!(null));
    assert!(out["output"].as_str().unwrap().contains("started"), "{out}");
}
