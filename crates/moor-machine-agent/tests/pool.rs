//! `lease` and `dial` on a pool host, against a fake crabbox.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

mod fake_crabbox;

/// Runs the agent with `args` and `--pool fake` in `home`.
fn agent(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor-machine-agent"));
    let crabbox = home.join("crabbox");
    cmd.args(args)
        .args(["--pool", "fake", "--crabbox"])
        .arg(crabbox)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME");
    cmd
}

/// `lease ARGS`: whether it succeeded and the answer it printed.
fn lease(home: &Path, args: &[&str]) -> (bool, Value) {
    let mut all = vec!["lease"];
    all.extend(args);
    let out: Output = agent(home, &all).output().unwrap();
    let printed = String::from_utf8(out.stdout).unwrap();
    let value = serde_json::from_str(&printed).unwrap_or(Value::Null);
    (out.status.success(), value)
}

fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    fake_crabbox::install(home.path());
    home
}

/// A home with lease moor-7 warmed, as every verb but warmup needs.
fn warmed() -> tempfile::TempDir {
    let home = home();
    assert!(lease(home.path(), &["warmup", "moor-7"]).0);
    home
}

const DONE: fn() -> Value = || json!({"reply": "leased", "answer": "done"});

#[test]
fn each_verb_runs_crabbox_with_the_agents_provider_from_the_lease_dir() {
    let home = home();
    for verb in ["warmup", "pause", "resume", "stop"] {
        assert_eq!(
            lease(home.path(), &[verb, "moor-7"]),
            (true, DONE()),
            "{verb}"
        );
    }
    let dir = home.path().join("moor-pool/7/lease");
    let want = format!(
        "{0} warmup --provider fake --slug moor-7\n{0} pause --provider fake --id moor-7\n\
         {0} resume --provider fake --id moor-7\n{0} stop --provider fake --id moor-7\n",
        dir.display()
    );
    assert_eq!(fake_crabbox::calls(home.path()), want);
    assert!(!dir.exists(), "stop removes the lease dir");
}

#[test]
fn stopping_a_lease_already_gone_succeeds() {
    let home = home();
    std::fs::write(home.path().join("stop-gone"), "").unwrap();
    assert_eq!(lease(home.path(), &["stop", "moor-7"]), (true, DONE()));
    std::fs::remove_file(home.path().join("stop-gone")).unwrap();
    std::fs::write(home.path().join("stop-missing"), "").unwrap();
    assert_eq!(
        lease(home.path(), &["stop", "moor-7"]),
        (true, DONE()),
        "external's not-found"
    );
}

#[test]
fn pausing_on_a_provider_that_cannot_answers_unsupported() {
    let home = warmed();
    std::fs::write(home.path().join("no-pause"), "").unwrap();
    let want = json!({"reply": "leased", "answer": "unsupported"});
    assert_eq!(lease(home.path(), &["pause", "moor-7"]), (true, want));
}

#[test]
fn info_gives_the_login_user_and_host_key_only() {
    let home = warmed();
    let want = json!({"reply": "leased", "answer": "info", "user": "vm",
        "host_keys": [fake_crabbox::HOST_KEY]});
    assert_eq!(lease(home.path(), &["info", "moor-7"]), (true, want));
}

#[test]
fn info_refuses_words_that_turn_host_key_checking_off() {
    let home = warmed();
    std::fs::write(home.path().join("loose"), "").unwrap();
    let (ok, answer) = lease(home.path(), &["info", "moor-7"]);
    assert!(!ok);
    assert_eq!(answer["reply"], "refused");
    assert!(answer["reason"].as_str().unwrap().contains("checking off"));
}

#[test]
fn a_slug_other_than_moor_id_never_reaches_crabbox() {
    let home = home();
    for slug in [
        "moor-7;id",
        "moor-07",
        "moor-",
        "moor--1",
        "box-7",
        "../moor-7",
    ] {
        let (ok, answer) = lease(home.path(), &["warmup", slug]);
        assert!(!ok, "{slug}");
        assert_eq!(answer["reply"], "refused", "{slug}");
    }
    let (ok, _) = lease(home.path(), &["list", "moor-7"]);
    assert!(!ok, "list takes no slug");
    assert_eq!(fake_crabbox::calls(home.path()), "");
}

#[test]
fn list_gives_the_moor_leases_and_their_states() {
    let home = home();
    let leases = json!([{"labels": {"slug": "moor-7"}, "state": "running"},
        {"slug": "moor-8", "status": "paused"}, {"name": "other", "state": "running"}]);
    std::fs::write(home.path().join("leases.json"), leases.to_string()).unwrap();
    let want = json!({"reply": "leased", "answer": "leases", "leases": [
        {"slug": "moor-7", "state": "running"}, {"slug": "moor-8", "state": "paused"}]});
    assert_eq!(lease(home.path(), &["list"]), (true, want));
    let log = fake_crabbox::calls(home.path());
    assert!(log.ends_with(" list --provider fake --json\n"), "{log}");
}

#[test]
fn pool_max_refuses_a_warmup_past_it() {
    let home = home();
    let leases = json!([{"slug": "moor-7"}, {"slug": "moor-8"}]);
    std::fs::write(home.path().join("leases.json"), leases.to_string()).unwrap();
    let (ok, answer) = lease(home.path(), &["warmup", "moor-9", "--pool-max", "2"]);
    assert!(!ok);
    assert_eq!(answer["reply"], "refused");
    assert!(!fake_crabbox::calls(home.path()).contains("warmup"));
    let again = lease(home.path(), &["warmup", "moor-8", "--pool-max", "2"]);
    assert_eq!(again, (true, DONE()), "a lease already counted may warm up");
}

#[test]
fn dial_carries_bytes_both_ways_through_the_proxy_command() {
    let home = warmed();
    let mut child = agent(home.path(), &["dial", "moor-7"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"SSH-2.0-hello\n")
        .unwrap();
    let mut back = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut back)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(back, "SSH-2.0-hello\n");
}

#[test]
fn a_pool_host_cannot_also_allow_exec() {
    let home = home();
    let out = agent(home.path(), &["run", "--allow-exec"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("--allow-exec"), "{err}");
}
