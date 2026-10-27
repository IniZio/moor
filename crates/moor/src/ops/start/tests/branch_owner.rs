use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::{BoxId, ErrorKind};

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

#[test]
fn a_branch_that_belongs_to_another_open_box_is_refused() {
    let w = world(FakeDriver::new());
    let a = run(&w.cx, &req("alpha")).unwrap();
    let store = w.cx.store().unwrap();
    let taken = Fields {
        branch: Field::Set("moor/beta".to_owned()),
        ..Fields::default()
    };
    store
        .apply(Decision::new(BoxId::new(a.boxed.id)).set(taken))
        .unwrap();
    let err = run(&w.cx, &req("beta")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(
        err.error
            .to_string()
            .contains("the branch moor/beta belongs to open Box alpha"),
        "{}",
        err.error
    );
    assert_eq!(store.open_boxes().unwrap().len(), 1);
}

#[test]
fn start_records_the_box_branch_and_makes_no_worktree() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &super::agentless_req("demo")).unwrap();
    assert_eq!(v.boxed.branch.as_deref(), Some("moor/demo"));
    let refs =
        w.cx.store()
            .unwrap()
            .box_refs(BoxId::new(v.boxed.id), "branch");
    assert_eq!(refs.unwrap(), vec!["moor/demo".to_string()]);
    assert_eq!(w.surface.calls(), ["ensure_workspace moor:demo"]);
}
