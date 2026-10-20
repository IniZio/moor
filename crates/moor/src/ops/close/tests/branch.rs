//! Close brings the kept worktree and Task branch up to the remote tip, and only by fast-forward.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::proc::Tool;
use moor_core::BoxPath;

use super::{admit, req, run, Fx, HEAD};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const BRANCH: &str = "moor/t";
const REMOTE: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// Git answers shared by every scenario: nothing unpushed, HEAD on the Task branch.
fn base(status: &str) -> ScriptedProc {
    scripted(status, true)
}

fn scripted(status: &str, pushed: bool) -> ScriptedProc {
    let p = ScriptedProc::new();
    p.on(Tool::Git, &["rev-list"], Reply::Ok("0\n".to_owned()));
    let remote = if pushed {
        Reply::Ok(format!("{REMOTE}\trefs/heads/{BRANCH}\n"))
    } else {
        Reply::Fail {
            code: 2,
            stdout: String::new(),
            stderr: String::new(),
        }
    };
    p.on(Tool::Git, &["ls-remote"], remote);
    p.on(Tool::Git, &["status"], Reply::Ok(status.to_owned()));
    p.on(
        Tool::Git,
        &["symbolic-ref"],
        Reply::Ok(format!("{BRANCH}\n")),
    );
    p.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(format!("{HEAD}\n")),
    );
    p.on(
        Tool::Git,
        &["rev-parse", "refs/remotes/origin/moor/t"],
        Reply::Ok(format!("{REMOTE}\n")),
    );
    p.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/main/.git\n".to_owned()),
    );
    p
}

fn ancestor(p: &ScriptedProc, old: &str, new: &str, is: bool) {
    let reply = if is {
        Reply::Ok(String::new())
    } else {
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }
    };
    p.on(Tool::Git, &["merge-base", "--is-ancestor", old, new], reply);
}

fn fx(proc: ScriptedProc) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().peek_text(
        "box-1",
        &BoxPath::under("/work/box-1", ".moor/boxwork").unwrap(),
        "ahead=0 dirty=0",
    ));
    let surface = Arc::new(FakeSurface::new("/s"));
    let proc = Arc::new(proc);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let id = admit(&cx, &driver, dir.path());
    let fields = Fields {
        branch: Field::Set(BRANCH.to_owned()),
        ..Fields::default()
    };
    cx.store()
        .unwrap()
        .apply(Decision::new(id).set(fields))
        .unwrap();
    Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    }
}

fn merged(f: &Fx) -> bool {
    f.proc.calls().iter().any(|c| {
        c.args.first().is_some_and(|a| a == "merge") && c.args.contains(&"--ff-only".to_owned())
    })
}

fn warning_codes(f: &Fx) -> Vec<String> {
    f.cx.take_warnings()
        .into_iter()
        .map(|w| w.code)
        .filter(|c| c == "branch_not_synced")
        .collect()
}

#[test]
fn a_remote_tip_ahead_of_the_branch_is_fast_forwarded() {
    let p = base("");
    ancestor(&p, REMOTE, HEAD, false);
    ancestor(&p, HEAD, REMOTE, true);
    p.on(Tool::Git, &["fetch"], Reply::Ok(String::new()));
    p.on(Tool::Git, &["merge"], Reply::Ok(String::new()));
    let f = fx(p);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.lifecycle, BoxLifecycle::Closed);
    assert!(merged(&f));
    assert!(warning_codes(&f).is_empty());
    let fetched = f.proc.calls().iter().any(|c| {
        c.args
            .starts_with(&["fetch".to_owned(), "origin".to_owned()])
    });
    assert!(fetched);
}

#[test]
fn a_diverged_branch_is_left_alone_with_a_warning() {
    let p = base("");
    ancestor(&p, REMOTE, HEAD, false);
    ancestor(&p, HEAD, REMOTE, false);
    p.on(Tool::Git, &["fetch"], Reply::Ok(String::new()));
    let f = fx(p);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.lifecycle, BoxLifecycle::Closed);
    assert!(!merged(&f));
    assert_eq!(warning_codes(&f), ["branch_not_synced"]);
    assert!(v.worktree_kept);
    assert!(!f
        .surface
        .calls()
        .iter()
        .any(|c| c.starts_with("remove_worktree")));
}

#[test]
fn a_dirty_worktree_is_left_alone_with_a_warning() {
    let p = base(" M a.txt\n");
    ancestor(&p, REMOTE, HEAD, false);
    ancestor(&p, HEAD, REMOTE, true);
    p.on(Tool::Git, &["fetch"], Reply::Ok(String::new()));
    let f = fx(p);
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.task.lifecycle, BoxLifecycle::Closed);
    assert!(!merged(&f));
    assert_eq!(warning_codes(&f), ["branch_not_synced"]);
}

#[test]
fn a_failed_fetch_warns_and_close_still_succeeds() {
    let p = base("");
    p.on(
        Tool::Git,
        &["fetch"],
        Reply::Fail {
            code: 128,
            stdout: String::new(),
            stderr: "could not read from remote".to_owned(),
        },
    );
    let f = fx(p);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.lifecycle, BoxLifecycle::Closed);
    assert!(v.box_deleted);
    assert!(!merged(&f));
    assert_eq!(warning_codes(&f), ["branch_not_synced"]);
}

#[test]
fn a_branch_never_pushed_is_skipped_without_a_warning() {
    let p = scripted("", false);
    let f = fx(p);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.lifecycle, BoxLifecycle::Closed);
    assert!(v.box_deleted);
    assert!(!merged(&f));
    let fetched = f
        .proc
        .calls()
        .iter()
        .any(|c| c.args.first().is_some_and(|a| a == "fetch"));
    assert!(!fetched);
    assert!(warning_codes(&f).is_empty());
}
