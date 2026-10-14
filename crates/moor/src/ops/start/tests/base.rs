use moor_core::seam::proc::Tool;

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

fn ok(v: &str) -> Reply {
    Reply::Ok(format!("{v}\n"))
}

fn follow_origin(w: &super::World) {
    w.proc.on(
        Tool::Git,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        ok("origin/main"),
    );
}

fn fetched(w: &super::World) -> bool {
    w.proc
        .calls()
        .iter()
        .any(|c| c.tool == Tool::Git && c.args == ["fetch", "origin", "main"])
}

#[test]
fn a_fetched_default_branch_is_the_base() {
    let w = world(FakeDriver::new());
    follow_origin(&w);
    w.proc.on(Tool::Git, &["fetch", "origin", "main"], ok(""));
    w.proc
        .on(Tool::Git, &["rev-parse", "origin/main"], ok("c643cc8a"));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(fetched(&w));
    assert_eq!(w.surface.bases(), [Some("c643cc8a".to_owned())]);
    assert_eq!(v.task.base.as_deref(), Some("c643cc8a"));
    assert!(w.cx.take_warnings().is_empty());
}

#[test]
fn a_failed_fetch_falls_back_to_the_local_branch_with_a_warning() {
    let w = world(FakeDriver::new());
    follow_origin(&w);
    w.proc.on(
        Tool::Git,
        &["fetch", "origin", "main"],
        Reply::Fail {
            code: 128,
            stdout: String::new(),
            stderr: "could not resolve host".into(),
        },
    );
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(w.surface.bases(), [None]);
    assert_eq!(v.task.base.as_deref(), Some("abc123"));
    let warnings = w.cx.take_warnings();
    assert!(
        warnings.iter().any(|x| x.code == "fetch_failed"),
        "{warnings:?}"
    );
}

#[test]
fn an_explicit_base_is_not_fetched() {
    let w = world(FakeDriver::new());
    follow_origin(&w);
    let mut r = req("demo");
    r.base = Some("release".into());
    let v = run(&w.cx, &r).unwrap();
    assert!(!fetched(&w));
    assert_eq!(w.surface.bases(), [Some("release".to_owned())]);
    assert_eq!(v.task.base.as_deref(), Some("release"));
}

#[test]
fn a_repo_without_a_remote_default_is_not_fetched() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!fetched(&w));
    assert_eq!(w.surface.bases(), [None]);
    assert_eq!(v.task.base.as_deref(), Some("abc123"));
    assert!(w.cx.take_warnings().is_empty());
}
