//! Tests for the open pull request lookup.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::TaskInput;
use moor_core::seam::forge::{ForgeError, ForgeRepo};
use moor_core::seam::proc::Tool;
use moor_core::BoxName;

use super::open_pr_url;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const ORIGIN: &str = "git@github.com:o/r.git";
const URL: &str = "https://github.com/o/r/pull/7";

/// A context over `forge` whose worktree `origin` is `ORIGIN`.
fn cx(dir: &Path, forge: FakeForge) -> Cx {
    let proc = ScriptedProc::new();
    proc.on(
        Tool::Git,
        &["remote", "get-url", "origin"],
        Reply::Ok(format!("{ORIGIN}\n")),
    );
    Cx::fake(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(proc),
    )
    .with_forge(Arc::new(forge))
}

/// A forge that knows `ORIGIN` as `o/r`.
fn forge() -> FakeForge {
    FakeForge::new().with_repo(ORIGIN, ForgeRepo::new("o", "r"))
}

/// Records Task `t` on branch `feat` in worktree `/wt`.
fn task(cx: &Cx) {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let fields = Fields {
        branch: Field::Set("feat".to_owned()),
        worktree: Field::Set("/wt".to_owned()),
        ..Fields::default()
    };
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::Admitted)
                .set(fields),
        )
        .unwrap();
}

#[test]
fn an_open_pr_gives_its_url() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), forge().with_open_pr("o", "r", "feat", URL));
    task(&cx);
    assert_eq!(open_pr_url(&cx, "t"), Ok(Some(URL.to_owned())));
}

#[test]
fn an_unknown_task_has_no_url() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), forge().with_open_pr("o", "r", "feat", URL));
    assert_eq!(open_pr_url(&cx, "t"), Ok(None));
}

#[test]
fn a_branch_without_a_pr_has_no_url() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), forge().with_open_pr("o", "r", "other", URL));
    task(&cx);
    assert_eq!(open_pr_url(&cx, "t"), Ok(None));
}

#[test]
fn a_forge_failure_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(
        dir.path(),
        forge().with_pr_error(ForgeError::Network("down".to_owned())),
    );
    task(&cx);
    let err = open_pr_url(&cx, "t").unwrap_err();
    assert!(err.contains("down"), "{err}");
}
