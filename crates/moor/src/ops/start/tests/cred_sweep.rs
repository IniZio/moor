use std::sync::Arc;

use moor_core::StartReq;

use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

#[test]
fn a_start_sweeps_stale_credential_dirs_before_launching() {
    let engine = Arc::new(FakeEngine::new());
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let goal = StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &goal).unwrap();
    let sweeps = w.driver.sweeps();
    assert_eq!(sweeps.len(), 1);
    let script = sweeps.first().and_then(|s| s.1.argv.get(2)).unwrap();
    assert!(script.contains(".moor-cred-"));
    assert!(w.driver.calls().iter().any(|c| c.starts_with("sweep ")));
    assert_eq!(engine.launches().len(), 1);
}
