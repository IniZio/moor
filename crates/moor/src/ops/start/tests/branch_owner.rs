use moor_core::seam::surface::Worktree;
use moor_core::{BoxId, ErrorKind};

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

#[test]
fn a_worktree_that_belongs_to_another_open_box_is_refused() {
    let w = world(FakeDriver::new());
    let a = run(&w.cx, &req("alpha")).unwrap();
    let path = a.task.worktree.unwrap();
    w.surface.set_next_worktree(Worktree {
        path: path.clone().into(),
        branch: "alpha".to_owned(),
    });
    let err = run(&w.cx, &req("beta")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(
        err.error.to_string().contains("belongs to open Box alpha"),
        "{}",
        err.error.to_string()
    );
    let open = w.cx.store().unwrap().open_tasks().unwrap();
    let bound = open
        .iter()
        .filter(|t| t.worktree.as_deref() == Some(path.as_str()))
        .count();
    assert_eq!(bound, 1);
}

#[test]
fn start_records_branch_ref() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    let refs = w.cx.store().unwrap().box_refs(BoxId::new(v.task.id), "branch");
    assert_eq!(refs.unwrap(), vec!["demo".to_string()]);
}
