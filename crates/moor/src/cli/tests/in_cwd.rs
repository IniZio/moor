//! The CLI resolves every cwd-relative part of a request before it sends it.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::path::Path;

use serde_json::{json, Value};

use super::argv;
use crate::cli::{in_cwd, parse, Parsed};

const CWD: &str = "/home/me/b";

/// The request `args` parse to, resolved against [`CWD`], as JSON.
fn sent(args: &[&str]) -> Value {
    let Parsed::Request(inv) = parse(argv(args)) else {
        unreachable!("expected a request")
    };
    serde_json::to_value(in_cwd(inv.request, Path::new(CWD))).expect("json")
}

#[test]
fn a_start_with_no_repo_runs_in_the_callers_dir_with_an_absolute_worktree() {
    assert_eq!(
        sent(&["start", "demo", "--worktree", "../wt"]),
        json!({"cmd": "in", "dir": CWD, "request":
            {"cmd": "start", "name": "demo", "worktree": "/home/me/b/../wt",
             "no_agent": false, "hold": false, "resume": false, "trust_moor_yaml": false}}),
    );
}

#[test]
fn a_start_with_a_repo_names_no_dir() {
    let got = sent(&["start", "demo", "--repo", "moor"]);
    assert_eq!(got["cmd"], "start", "{got}");
    assert_eq!(got["repo"], "moor", "{got}");
}

#[test]
fn a_relative_cp_destination_becomes_absolute_and_an_absolute_one_is_kept() {
    assert_eq!(
        sent(&["cp", "demo", "a.txt", "out/a.txt"])["dest"],
        "/home/me/b/out/a.txt"
    );
    assert_eq!(
        sent(&["cp", "demo", "a.txt", "/tmp/a.txt"])["dest"],
        "/tmp/a.txt"
    );
}

#[test]
fn env_runs_in_the_callers_dir_or_on_an_absolute_repo_path() {
    assert_eq!(
        sent(&["env"]),
        json!({"cmd": "in", "dir": CWD, "request": {"cmd": "env"}})
    );
    assert_eq!(
        sent(&["env", "sub"]),
        json!({"cmd": "env", "repo": "/home/me/b/sub"})
    );
}

#[test]
fn other_requests_pass_through() {
    assert_eq!(sent(&["show", "demo"])["cmd"], "show");
}
