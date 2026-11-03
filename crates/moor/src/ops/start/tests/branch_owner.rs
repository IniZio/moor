use moor_core::{BoxId, BoxName, CloseReq};

use super::{agentless_req, world};
use crate::ops::{close, start::run};
use crate::testing::driver::FakeDriver;

#[test]
fn start_records_the_box_branch_by_its_id_and_makes_no_worktree() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &agentless_req("demo")).unwrap();
    let want = format!("moor/{}-demo", v.boxed.id);
    assert_eq!(v.boxed.branch.as_deref(), Some(want.as_str()));
    let refs =
        w.cx.store()
            .unwrap()
            .box_refs(BoxId::new(v.boxed.id), "branch");
    assert_eq!(refs.unwrap(), vec![want]);
    assert_eq!(w.surface.calls(), ["ensure_workspace moor:demo"]);
}

#[test]
fn a_name_reused_after_its_box_closed_gets_a_new_branch() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &agentless_req("demo")).unwrap();
    let req = CloseReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        force: true,
    };
    close::run(&w.cx, &req).unwrap();
    let second = run(&w.cx, &agentless_req("demo")).unwrap();
    assert_ne!(first.boxed.id, second.boxed.id);
    assert_ne!(first.boxed.branch, second.boxed.branch);
    let want = format!("moor/{}-demo", second.boxed.id);
    assert_eq!(second.boxed.branch.as_deref(), Some(want.as_str()));
}
