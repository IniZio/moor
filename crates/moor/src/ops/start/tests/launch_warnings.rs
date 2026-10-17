use std::sync::Arc;

use moor_core::exec_env::{build, BoxCtx, MacTask, Purpose};
use moor_core::seam::engine::LaunchNotice;
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxPath, StartReq};

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
        goal: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &start).unwrap();
    let got: Vec<_> = (w.cx.take_warnings().into_iter())
        .filter(|x| x.code == "test_notice")
        .map(|x| (x.code, x.message))
        .collect();
    assert_eq!(got, vec![(notice.code, notice.message)]);
}

/// Box release matches `MOOR_BOX_TAG` as `^<task>-[0-9a-f]{16}$`, so a Task `app` never
/// matches Task `app-two`. The nonce a start hands the agent launch must make the macOS Box
/// tag that form, which a 6-hex acquire nonce does not.
#[test]
fn the_agent_launch_nonce_makes_the_macos_tag_the_task_name_and_sixteen_lowercase_hex() {
    let engine = Arc::new(FakeEngine::new());
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let started = StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    crate::ops::start::run(&w.cx, &started).unwrap();
    let launch = engine.launches().remove(0);

    let key = TaskKey::new(&launch.task);
    let root = BoxPath::restore("/Users/Shared/moor").unwrap();
    let layout = MacLayout::new(&root, &key).unwrap();
    let ctx = BoxCtx {
        env: &[],
        proxy: None,
        origin: None,
        workdir: None,
        strip: &[],
        mac: Some(MacTask {
            layout: &layout,
            key: &key,
            nonce: &launch.nonce,
            toolchain_env: &[],
            toolchain_bin: &[],
        }),
    };
    let vars = build(&ctx, Purpose::Agent).unwrap().vars;
    let tag = vars
        .iter()
        .find(|(n, _)| n == "MOOR_BOX_TAG")
        .map(|(_, v)| v.as_str())
        .expect("the macOS agent env carries MOOR_BOX_TAG");

    let nonce = tag
        .strip_prefix("demo-")
        .expect("the tag starts with the Task name");
    assert!(
        nonce.len() == 16
            && nonce
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "{tag}"
    );
}
