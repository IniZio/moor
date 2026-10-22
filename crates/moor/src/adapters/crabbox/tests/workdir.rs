//! Tests that a Box's workdir comes only from what crabbox reports at acquire.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use super::{fixture, req, Reply};

#[test]
fn acquire_without_a_ready_line_fails_closed_and_never_searches_the_box() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_X slug=moor-t7-ab12cd\n".to_owned()),
    );
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Ok("ssh -o StrictHostKeyChecking=yes u@h\n".to_owned()),
    );
    p.on(Tool::Ssh, &[], Reply::Ok("R 0 0\n\n".to_owned()));
    let err = c.acquire(&req("sprites", None)).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    let calls = p.calls();
    assert_eq!(calls.len(), 1, "the warmup only: {calls:?}");
}

#[test]
fn a_workdir_lookup_without_a_recorded_one_fails_closed_and_runs_nothing() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Ok("ssh -o StrictHostKeyChecking=yes u@h\n".to_owned()),
    );
    p.on(Tool::Ssh, &[], Reply::Ok("R 0 0\n\n".to_owned()));
    let err = c
        .workdir(&BoxHandle::new("sprites/moor-t7-ab12cd"))
        .unwrap_err();
    assert!(err.to_string().contains("no recorded workdir"), "{err:?}");
    assert!(p.calls().is_empty(), "{:?}", p.calls());
}
