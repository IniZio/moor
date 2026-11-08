//! A get tells the runner whether the Box is still open, record or none.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::request::MachineBoxOp;
use moor_core::{BoxId, BoxName};

use crate::cx::Cx;
use crate::testing::cx::new_box;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

#[test]
fn a_get_says_a_closed_or_unknown_box_is_closed_and_an_open_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("a").unwrap();
    let id = store.apply(new_box(name)).unwrap().box_id;
    let closed = |box_id| {
        super::run(&cx, MachineBoxOp::Get { box_id })
            .unwrap()
            .closed
    };
    assert!(!closed(id), "open");
    assert!(closed(BoxId::new(id.get() + 1)), "never existed");
    let close = Decision::new(id).lifecycle(BoxLifecycleInput::Close { reason: None });
    store.apply(close).unwrap();
    assert!(closed(id), "closed");
}
