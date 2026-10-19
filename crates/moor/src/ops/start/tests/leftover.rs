use std::path::Path;

use moor_core::seam::proc::Tool;
use moor_core::seam::surface::SurfaceError;
use moor_core::ErrorKind;

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567\n";

fn ok(s: &str) -> Reply {
    Reply::Ok(s.to_owned())
}

fn failing() -> Reply {
    Reply::Fail {
        code: 128,
        stdout: String::new(),
        stderr: "fatal: bad revision".to_owned(),
    }
}

fn leftover(path: &Path) -> SurfaceError {
    SurfaceError::Leftover {
        path: path.to_path_buf(),
    }
}

#[test]
fn a_clean_leftover_worktree_is_removed_and_recreated_from_the_base() {
    let w = world(FakeDriver::new());
    let stale = tempfile::tempdir().unwrap();
    w.surface.set_fail_create(leftover(stale.path()));
    w.surface.add_worktree(stale.path());
    w.proc.on(Tool::Git, &["rev-list"], ok("0\n"));
    w.proc.on(Tool::Git, &["status"], ok(""));
    w.proc
        .before(Tool::Git, &["rev-parse", "--verify"], ok(SHA));
    let request = crate::ops::start::StartReq {
        base: Some("main".into()),
        ..req("demo")
    };
    run(&w.cx, &request).unwrap();
    assert_eq!(
        w.surface.calls(),
        [
            "create_worktree demo".to_owned(),
            format!("remove_worktree {}", stale.path().display()),
            "create_worktree demo".to_owned(),
            "ensure_workspace moor:demo".to_owned(),
        ]
    );
    assert_eq!(
        w.surface.bases(),
        [Some(SHA.trim().to_owned()), Some(SHA.trim().to_owned())]
    );
}

#[test]
fn an_explicit_base_is_recorded_and_branched_from_as_its_commit() {
    let w = world(FakeDriver::new());
    w.proc
        .before(Tool::Git, &["rev-parse", "--verify"], ok(SHA));
    let request = crate::ops::start::StartReq {
        base: Some("main".into()),
        ..req("demo")
    };
    let v = run(&w.cx, &request).unwrap();
    let stored =
        w.cx.store()
            .unwrap()
            .task(moor_core::BoxId::new(v.box_row.id))
            .unwrap()
            .unwrap();
    assert_eq!(stored.base.as_deref(), Some(SHA.trim()));
    assert_eq!(w.surface.bases(), [Some(SHA.trim().to_owned())]);
}

#[test]
fn an_unknown_base_is_refused_without_creating_a_task() {
    let w = world(FakeDriver::new());
    w.proc
        .before(Tool::Git, &["rev-parse", "--verify"], failing());
    let request = crate::ops::start::StartReq {
        base: Some("nope".into()),
        ..req("demo")
    };
    let err = run(&w.cx, &request).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(w.surface.calls().is_empty());
    assert!(w.cx.store().unwrap().open_tasks().unwrap().is_empty());
}

#[test]
fn a_leftover_worktree_with_unpushed_commits_refuses_the_start() {
    let w = world(FakeDriver::new());
    let stale = tempfile::tempdir().unwrap();
    w.surface.set_fail_create(leftover(stale.path()));
    w.proc.on(Tool::Git, &["rev-list"], ok("2\n"));
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let text = err.error.to_string();
    assert!(text.contains(&*stale.path().to_string_lossy()), "{text}");
    assert!(text.contains("2 unpushed"), "{text}");
    assert!(err.hints.iter().any(|h| h.contains("another Task name")));
    assert!(!w.surface.calls().iter().any(|c| c.starts_with("remove")));
}

#[test]
fn a_leftover_worktree_with_uncommitted_files_refuses_the_start() {
    let w = world(FakeDriver::new());
    let stale = tempfile::tempdir().unwrap();
    w.surface.set_fail_create(leftover(stale.path()));
    w.proc.on(Tool::Git, &["rev-list"], ok("0\n"));
    w.proc.on(Tool::Git, &["status"], ok(" M a.txt\n"));
    let err = run(&w.cx, &req("demo")).unwrap_err();
    let text = err.error.to_string();
    assert!(text.contains("uncommitted"), "{text}");
    assert!(text.contains(&*stale.path().to_string_lossy()), "{text}");
    assert!(!w.surface.calls().iter().any(|c| c.starts_with("remove")));
}

#[test]
fn a_leftover_worktree_whose_unpushed_check_fails_refuses_the_start() {
    let w = world(FakeDriver::new());
    let stale = tempfile::tempdir().unwrap();
    w.surface.set_fail_create(leftover(stale.path()));
    w.proc.on(Tool::Git, &["rev-list"], failing());
    let err = run(&w.cx, &req("demo")).unwrap_err();
    let text = err.error.to_string();
    assert!(text.contains("unpushed"), "{text}");
    assert!(text.contains(&*stale.path().to_string_lossy()), "{text}");
    assert!(!w.surface.calls().iter().any(|c| c.starts_with("remove")));
}

#[test]
fn a_leftover_worktree_whose_dirty_check_fails_refuses_the_start() {
    let w = world(FakeDriver::new());
    let stale = tempfile::tempdir().unwrap();
    w.surface.set_fail_create(leftover(stale.path()));
    w.proc.on(Tool::Git, &["rev-list"], ok("0\n"));
    w.proc.on(Tool::Git, &["status"], failing());
    let err = run(&w.cx, &req("demo")).unwrap_err();
    let text = err.error.to_string();
    assert!(text.contains("uncommitted"), "{text}");
    assert!(text.contains(&*stale.path().to_string_lossy()), "{text}");
    assert!(!w.surface.calls().iter().any(|c| c.starts_with("remove")));
}
