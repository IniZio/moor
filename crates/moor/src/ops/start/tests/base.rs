use std::path::Path;

use moor_core::seam::proc::Tool;
use moor_core::seam::surface::Surface;
use moor_core::BoxName;

use super::moor_yaml::script_base_yaml;
use super::setup::{execs, is_setup_line, setup_execs};
use super::{req, world, world_at};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

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
    assert_eq!(v.box_row.base.as_deref(), Some("c643cc8a"));
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
    assert_eq!(v.box_row.base.as_deref(), Some("abc123"));
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
    w.proc
        .before(Tool::Git, &["rev-parse", "--verify"], ok(SHA));
    let mut r = req("demo");
    r.base = Some("release".into());
    let v = run(&w.cx, &r).unwrap();
    assert!(!fetched(&w));
    assert_eq!(w.surface.bases(), [Some(SHA.to_owned())]);
    assert_eq!(v.box_row.base.as_deref(), Some(SHA));
}

#[test]
fn a_repo_without_a_remote_default_is_not_fetched() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!fetched(&w));
    assert_eq!(w.surface.bases(), [None]);
    assert_eq!(v.box_row.base.as_deref(), Some("abc123"));
    assert!(w.cx.take_warnings().is_empty());
}

#[test]
fn setup_comes_from_the_base_commit_not_the_checkout_file() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [host-only-line]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    follow_origin(&w);
    w.proc.on(Tool::Git, &["fetch", "origin", "main"], ok(""));
    w.proc
        .on(Tool::Git, &["rev-parse", "origin/main"], ok("c643cc8a"));
    w.proc.before(
        Tool::Git,
        &["ls-tree", "--name-only", "c643cc8a", "--", ".moor.yaml"],
        ok(".moor.yaml"),
    );
    w.proc.before(
        Tool::Git,
        &["show", "c643cc8a:.moor.yaml"],
        Reply::Ok("setup: [base-line]\n".into()),
    );
    run(&w.cx, &req("demo")).unwrap();
    let lines: Vec<String> = setup_execs(&w)
        .into_iter()
        .filter(|e| is_setup_line(&e.1.argv))
        .map(|e| e.1.argv.join(" "))
        .collect();
    assert!(lines.iter().any(|l| l.contains("base-line")), "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains("host-only-line")),
        "{lines:?}"
    );
    let warnings = w.cx.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|x| x.code == "moor_yaml_differs_from_base" && x.message.contains("c643cc8a")),
        "{warnings:?}"
    );
}

/// The text of every setup-line exec.
fn setup_lines(w: &super::World) -> Vec<String> {
    setup_execs(w)
        .into_iter()
        .filter(|e| is_setup_line(&e.1.argv))
        .map(|e| e.1.argv.join(" "))
        .collect()
}

fn differs_warned(w: &super::World) -> bool {
    (w.cx.take_warnings().iter()).any(|x| x.code == "moor_yaml_differs_from_base")
}

#[test]
fn a_checkout_file_equal_to_the_base_copy_runs_without_a_warning() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [same-line]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    script_base_yaml(&w, Some("setup: [same-line]\n"));
    run(&w.cx, &req("demo")).unwrap();
    let lines = setup_lines(&w);
    assert!(lines.iter().any(|l| l.contains("same-line")), "{lines:?}");
    assert!(!differs_warned(&w));
}

#[test]
fn an_unreadable_base_copy_fails_the_start_before_any_box_work() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [host-only-line]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    w.proc.before(
        Tool::Git,
        &["ls-tree"],
        Reply::Fail {
            code: 128,
            stdout: String::new(),
            stderr: "bad object".into(),
        },
    );
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert!(
        err.error
            .to_string()
            .contains("cannot read .moor.yaml at base commit"),
        "{err:?}"
    );
    assert!(execs(&w).is_empty());
}

#[test]
fn a_bound_worktree_keeps_its_own_file_over_a_different_base_copy() {
    let w = world(FakeDriver::new());
    let made = w
        .surface
        .create_worktree(
            Path::new("/repo"),
            &BoxName::parse("repo/pre").unwrap(),
            None,
        )
        .unwrap();
    std::fs::create_dir_all(&made.path).unwrap();
    std::fs::write(made.path.join(".moor.yaml"), "setup: [worktree-line]\n").unwrap();
    script_base_yaml(&w, Some("setup: [base-line]\n"));
    let mut r = req("demo");
    r.worktree = Some(made.path.display().to_string());
    run(&w.cx, &r).unwrap();
    let lines = setup_lines(&w);
    assert!(
        lines.iter().any(|l| l.contains("worktree-line")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("base-line")), "{lines:?}");
    assert!(!differs_warned(&w));
}
