use std::sync::Arc;

use moor_core::seam::engine::LaunchNotice;
use moor_core::StartReq;

use super::relaunch::session;
use super::{req, world};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

#[test]
fn launch_notices_are_reported_as_warnings() {
    let notice = LaunchNotice {
        code: "test_notice".to_owned(),
        message: "a test notice".to_owned(),
    };
    let launched = moor_core::seam::engine::SessionRef {
        warnings: vec![notice.clone()],
        ..session("p-1")
    };
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(launched)));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine);
    let start = StartReq {
        prompt: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &start).unwrap();
    let got: Vec<_> = (w.cx.take_warnings().into_iter())
        .filter(|x| x.code == "test_notice")
        .map(|x| (x.code, x.message))
        .collect();
    assert_eq!(got, vec![(notice.code, notice.message)]);
}
