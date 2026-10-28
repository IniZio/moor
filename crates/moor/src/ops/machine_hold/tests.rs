//! Only the operator sends `machine-hold`: a local caller who is not the operator is refused,
//! even for its own Box.

use std::sync::Arc;

use moor_core::user::{Caller, Identity};
use moor_core::{BoxName, ErrorKind, HoldOp, MachineHoldReq, Request};

use crate::cx::Cx;
use crate::ops::authz::{authorize, HOLD_OPERATOR_ONLY};
use crate::testing::cx::new_box;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

#[test]
fn a_local_caller_who_is_not_the_operator_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&Identity::local(1000), None).unwrap();
    let name = BoxName::parse_allow_bare("a").unwrap();
    let id = store.apply(new_box(name).requester(user)).unwrap().box_id;
    let hold = Request::MachineHold(MachineHoldReq {
        id,
        op: HoldOp::repo(),
    });
    let err = authorize(&cx, &Ok(Caller::Local(1000)), None, &hold).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), HOLD_OPERATOR_ONLY);
}
