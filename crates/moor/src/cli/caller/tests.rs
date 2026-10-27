//! The CLI fills the repo from its caller's checkout and refuses one moor cannot clone.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use moor_core::seam::proc::Tool;
use serde_json::{json, Value};

use super::{resolve, Resolved, NOT_GITHUB, REPO_ARG};
use crate::adapters::git::Git;
use crate::cli::tests::argv;
use crate::cli::{parse, Parsed};
use crate::testing::proc::{Reply, ScriptedProc};

const CWD: &str = "/home/me/b";

/// A git whose checkout at [`CWD`] has `origin` and, when `dirty`, local changes.
fn git(origin: Option<&str>, dirty: bool) -> (Arc<ScriptedProc>, Git) {
    let proc = Arc::new(ScriptedProc::new());
    let url = origin.map_or_else(
        || Reply::Fail {
            code: 2,
            stdout: String::new(),
            stderr: "error: No such remote 'origin'".into(),
        },
        |o| Reply::Ok(format!("{o}\n")),
    );
    proc.on(Tool::Git, &["remote", "get-url", "origin"], url);
    let status = if dirty {
        "## main...origin/main\n M a.rs\n"
    } else {
        "## main...origin/main\n"
    };
    proc.on(Tool::Git, &["status"], Reply::Ok(status.into()));
    (Arc::clone(&proc), Git::new(proc))
}

/// What `args` resolve to in a checkout with `origin`.
fn resolved(args: &[&str], origin: Option<&str>, dirty: bool) -> Result<Resolved, String> {
    let Parsed::Request(inv) = parse(argv(args)) else {
        unreachable!("expected a request")
    };
    resolve(inv.request, Path::new(CWD), &git(origin, dirty).1)
}

fn sent(args: &[&str], origin: Option<&str>) -> Value {
    let r = resolved(args, origin, false).expect("resolves");
    serde_json::to_value(r.request).expect("json")
}

#[test]
fn a_start_in_a_github_checkout_sends_its_owner_and_name() {
    for origin in [
        "git@github.com:o/n.git",
        "https://github.com/o/n",
        "ssh://git@github.com/o/n.git",
    ] {
        let got = sent(&["start", "n/1"], Some(origin));
        assert_eq!(got["repo"], "o/n", "{origin}: {got}");
        assert_eq!(got["cmd"], "start", "{got}");
    }
}

#[test]
fn a_start_outside_a_checkout_sends_no_repo() {
    assert_eq!(
        sent(&["start", "demo"], None),
        json!({"cmd": "start", "name": "demo", "no_agent": false, "resume": false}),
    );
}

#[test]
fn a_non_github_origin_is_refused_without_quoting_it() {
    for args in [&["start", "demo"][..], &["env"]] {
        let err = resolved(args, Some("https://u:tok3n@gitlab.com/o/n.git"), false).unwrap_err();
        assert_eq!(err, NOT_GITHUB, "{args:?}");
    }
}

#[test]
fn a_named_repo_skips_the_checkout() {
    let (proc, git) = git(Some("https://gitlab.com/x/y"), true);
    for (arg, want) in [
        ("o/n", "o/n"),
        ("https://github.com/a/b.git", "a/b"),
        ("git@github.com:c/d", "c/d"),
    ] {
        let Parsed::Request(inv) = parse(argv(&["start", "demo", "--repo", arg])) else {
            unreachable!()
        };
        let r = resolve(inv.request, Path::new(CWD), &git).unwrap();
        assert!(r.warning.is_none());
        assert_eq!(serde_json::to_value(r.request).unwrap()["repo"], want);
    }
    assert!(proc.calls().is_empty(), "{:?}", proc.calls());
}

#[test]
fn a_refused_repo_flag_never_echoes_its_value() {
    for args in [
        &[
            "start",
            "demo",
            "--repo",
            "https://u:tok3n@gitlab.com/o/n.git",
        ][..],
        &["env", "https://u:tok3n@gitlab.com/o/n.git"],
    ] {
        let Parsed::Done(out) = parse(argv(args)) else {
            unreachable!("{args:?} parsed")
        };
        assert_ne!(out.code, 0, "{args:?}");
        let shown = format!("{}{}", out.stdout, out.stderr);
        assert!(shown.contains(REPO_ARG), "{shown}");
        assert!(!shown.contains("tok3n"), "{shown}");
    }
}

#[test]
fn repo_flag_refuses_a_path_or_another_host() {
    for arg in ["/home/me/app", "app", "https://gitlab.com/o/n"] {
        assert!(
            !matches!(
                parse(argv(&["start", "demo", "--repo", arg])),
                Parsed::Request(_)
            ),
            "{arg}"
        );
    }
}

#[test]
fn local_changes_warn_that_the_box_starts_from_what_is_pushed() {
    let r = resolved(&["start", "demo"], Some("git@github.com:o/n.git"), true).unwrap();
    let w = r.warning.expect("warning");
    assert_eq!(w.code, "local_changes_not_in_box");
    assert!(w.message.contains("pushed to o/n"), "{}", w.message);
    let clean = resolved(&["start", "demo"], Some("git@github.com:o/n.git"), false).unwrap();
    assert!(clean.warning.is_none());
}

#[test]
fn env_and_start_needs_take_the_checkout_repo() {
    assert_eq!(
        sent(&["env"], Some("git@github.com:o/n.git")),
        json!({"cmd": "env", "repo": "o/n"})
    );
    let needs = moor_core::Request::StartNeeds(moor_core::request::StartNeedsReq {
        identity: None,
        repo: None,
    });
    let r = resolve(
        needs,
        Path::new(CWD),
        &git(Some("git@github.com:o/n.git"), false).1,
    );
    assert_eq!(
        serde_json::to_value(r.unwrap().request).unwrap()["repo"],
        "o/n"
    );
}

#[test]
fn a_relative_cp_destination_becomes_absolute_and_an_absolute_one_is_kept() {
    assert_eq!(
        sent(&["cp", "demo", "a.txt", "out/a.txt"], None)["dest"],
        "/home/me/b/out/a.txt"
    );
    assert_eq!(
        sent(&["cp", "demo", "a.txt", "/tmp/a.txt"], None)["dest"],
        "/tmp/a.txt"
    );
}

#[test]
fn a_real_checkout_resolves_through_host_git() {
    let dir = tempfile::tempdir().unwrap();
    let st = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(st.success());
    let st = Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:o/n.git"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(st.success());
    let Parsed::Request(inv) = parse(argv(&["start", "n/1"])) else {
        unreachable!()
    };
    let env = crate::env::Env::from_pairs([("PATH", std::env::var("PATH").unwrap_or_default())]);
    let r = resolve(inv.request, dir.path(), &crate::Moor::host_checkout(&env)).unwrap();
    assert_eq!(serde_json::to_value(r.request).unwrap()["repo"], "o/n");
}
