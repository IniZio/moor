//! Tests for `gate`.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::{BoxId, BoxName, ErrorKind, GateReq, Sha};

use super::run;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{NewRun, RunOutcome};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const SHA: &str = "abcdef1234567890abcdef1234567890abcdef12";
const OTHER: &str = "abcdef9999999999999999999999999999999999";

/// Context over a repo directory that is not a git checkout; git has no replies.
fn cx(dir: &Path) -> Cx {
    Cx::fake(
        dir,
        Path::new("/not/a/checkout"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
    )
}

/// Insert a task by name.
fn task(cx: &Cx, name: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .task
}

/// Record a finished run of `command` named `name` on `sha` with `exit`.
fn record(cx: &Cx, task: BoxId, name: &str, sha: &str, command: &str, exit: i32) {
    let store = cx.store().unwrap();
    let id = store
        .begin_run(NewRun {
            task,
            argv: command.split(' ').map(str::to_owned).collect(),
            record: Some(name.to_owned()),
            sha: Some(Sha::parse(sha).unwrap()),
            sync: "tree".into(),
            pid: 1,
        })
        .unwrap();
    let out = RunOutcome {
        exit: Some(exit),
        interrupted: false,
        error: None,
    };
    store.finish_run(id, out).unwrap();
}

/// A check of gate `test` on `sha`.
fn req(sha: &str, command: Option<&str>) -> GateReq {
    GateReq {
        sha: sha.to_owned(),
        name: "test".to_owned(),
        command: command.map(str::to_owned),
    }
}

/// A field of a response value; null when absent.
fn field(v: &serde_json::Value, key: &str) -> serde_json::Value {
    v.get(key).cloned().unwrap_or_default()
}

/// The error of a failed check.
fn err(cx: &Cx, r: &GateReq) -> CommandError {
    run(cx, r).unwrap_err()
}

#[test]
fn passing_run_is_ok_with_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "test", SHA, "make test", 0);
    let v = serde_json::to_value(run(&cx, &req("ABCDEF12", None)).unwrap()).unwrap();
    assert_eq!(field(&v, "ok"), true);
    assert_eq!(field(&v, "sha"), SHA);
    assert_eq!(field(&v, "name"), "test");
    assert_eq!(field(&v, "command"), "make test");
    assert_eq!(field(&v, "exit"), 0);
    assert_eq!(field(&v, "recorded_at"), "1970-01-01T00:00:01Z");
}

#[test]
fn works_without_a_git_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "test", SHA, "make test", 0);
    assert!(run(&cx, &req(SHA, None)).is_ok());
}

#[test]
fn failing_run_is_failed_and_keeps_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "test", SHA, "make test", 2);
    let e = err(&cx, &req(SHA, None));
    assert_eq!(e.kind, ErrorKind::Failed);
    let v = e.value.unwrap();
    assert_eq!(field(&v, "ok"), false);
    assert_eq!(field(&v, "exit"), 2);
    assert_eq!(field(&v, "sha"), SHA);
}

#[test]
fn known_commit_without_the_gate_is_failed_with_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "lint", SHA, "make lint", 0);
    let e = err(&cx, &req(SHA, None));
    assert_eq!(e.kind, ErrorKind::Failed);
    let v = e.value.unwrap();
    assert_eq!(field(&v, "ok"), false);
    assert_eq!(field(&v, "command"), serde_json::Value::Null);
}

#[test]
fn malformed_sha_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    assert_eq!(err(&cx, &req("xyz!", None)).kind, ErrorKind::Usage);
    assert_eq!(err(&cx, &req("abc", None)).kind, ErrorKind::Usage);
}

#[test]
fn unknown_short_prefix_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "test", SHA, "make test", 0);
    assert_eq!(err(&cx, &req("ffff", None)).kind, ErrorKind::Usage);
}

#[test]
fn ambiguous_prefix_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "test", SHA, "make test", 0);
    record(&cx, t, "test", OTHER, "make test", 0);
    assert_eq!(err(&cx, &req("abcdef", None)).kind, ErrorKind::Usage);
}

#[test]
fn unknown_long_prefix_is_failed() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let e = err(&cx, &req("ffffffff", None));
    assert_eq!(e.kind, ErrorKind::Failed);
}

#[test]
fn command_filter_matches_exact_text() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    record(&cx, t, "test", SHA, "make test", 0);
    record(&cx, t, "test", SHA, "make lint", 1);
    let v = run(&cx, &req(SHA, Some("make test"))).unwrap();
    assert!(v.ok);
    let e = err(&cx, &req(SHA, Some("make lint")));
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(field(&e.value.unwrap(), "command"), "make lint");
    let e = err(&cx, &req(SHA, Some("make")));
    assert_eq!(field(&e.value.unwrap(), "command"), serde_json::Value::Null);
}
