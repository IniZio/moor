//! Close removes the worktree moor created once its work is safe, and keeps it otherwise.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::proc::Tool;

use super::{admit, fx_with, req, row, run, Fx, HEAD};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

fn removed(f: &Fx) -> Vec<String> {
    f.surface
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("remove_worktree"))
        .collect()
}

fn warning_codes(f: &Fx) -> Vec<String> {
    f.cx.take_warnings().into_iter().map(|w| w.code).collect()
}

fn fx_status(status: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let surface = Arc::new(FakeSurface::new("/s"));
    let p = ScriptedProc::new();
    p.on(Tool::Git, &["rev-list"], Reply::Ok("0\n".to_owned()));
    p.on(Tool::Git, &["status"], Reply::Ok(status.to_owned()));
    p.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(format!("{HEAD}\n")),
    );
    p.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/main/.git\n".to_owned()),
    );
    let proc = Arc::new(p);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let id = admit(&cx, &driver, dir.path());
    Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    }
}

#[test]
fn a_clean_close_removes_the_worktree_before_closing_the_workspace() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    let v = run(&f.cx, &req(false)).unwrap();
    let calls = f.surface.calls();
    let at = |prefix: &str| calls.iter().position(|c| c.starts_with(prefix));
    let rm = at("remove_worktree ").expect("worktree removed");
    assert!(rm < at("close_workspace").expect("workspace closed"));
    assert!(!v.worktree_kept);
    assert!(warning_codes(&f).is_empty());
}

#[test]
fn a_forced_close_with_unpushed_work_keeps_the_worktree() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 2);
    let v = run(&f.cx, &req(true)).unwrap();
    assert!(removed(&f).is_empty() && v.worktree_kept);
    assert!(warning_codes(&f).contains(&"worktree_kept".to_owned()));
}

#[test]
fn a_forced_close_with_dirty_work_keeps_the_worktree() {
    let f = fx_status(" M a.txt\n");
    let v = run(&f.cx, &req(true)).unwrap();
    assert!(removed(&f).is_empty() && v.worktree_kept);
    assert!(warning_codes(&f).contains(&"worktree_kept".to_owned()));
}

#[test]
fn a_bound_worktree_is_never_removed() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    let fields = Fields {
        bound: Field::Set(true),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(fields))
        .unwrap();
    assert!(row(&f).bound);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(removed(&f).is_empty() && v.worktree_kept);
    assert!(f.dir.path().is_dir());
}

#[test]
fn a_worktree_already_gone_counts_as_removed() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(removed(&f).len(), 1);
    assert!(!v.worktree_kept);
    assert!(!warning_codes(&f).contains(&"worktree_remove_failed".to_owned()));
}
