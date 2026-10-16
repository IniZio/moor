//! A device kind that fails to provision is reported, and the Task still launches.

use std::sync::Arc;

use moor_core::seam::engine::DeviceFailure;
use moor_core::{StartReq, TaskId};
use serde_json::json;

use super::relaunch::session;
use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

fn goal_req(name: &str) -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req(name)
    }
}

fn with_engine(engine: &Arc<FakeEngine>) -> World {
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    w
}

#[test]
fn a_failed_device_is_recorded_and_warned_and_the_task_still_launches() {
    let mut launched = session("p-1");
    launched.device_failures = vec![DeviceFailure {
        kind: "ios".to_owned(),
        reason: "simctl create exited 1".to_owned(),
    }];
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(launched)));
    let w = with_engine(&engine);
    let v = run(&w.cx, &goal_req("fix-login")).unwrap();
    let id = TaskId::new(v.task.id);
    assert!(v.agent.launched);
    assert!(v.agent.reason.is_none());
    assert!(w.cx.store().unwrap().live_session(id).unwrap().is_some());
    assert_eq!(engine.launches().first().map(|l| l.task_id), Some(id));
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    let ev = events
        .iter()
        .find(|e| e.kind == "device_provision_failed")
        .expect("device_provision_failed event");
    assert_eq!(ev.payload.get("device"), Some(&json!("ios")));
    assert_eq!(
        ev.payload.get("reason"),
        Some(&json!("simctl create exited 1"))
    );
    let warnings = w.cx.take_warnings();
    let w = warnings
        .iter()
        .find(|w| w.code == "device_provision_failed")
        .expect("device_provision_failed warning");
    assert!(
        w.message.contains("ios") && w.message.contains("simctl create exited 1"),
        "{}",
        w.message
    );
}

#[test]
fn a_launch_without_device_failures_records_no_device_event() {
    let engine = Arc::new(FakeEngine::new());
    let w = with_engine(&engine);
    let v = run(&w.cx, &goal_req("fix-login")).unwrap();
    let id = TaskId::new(v.task.id);
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    assert!(events.iter().all(|e| e.kind != "device_provision_failed"));
}
