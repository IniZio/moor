//! A fresh work Box session records the contract version it read; a task's Box is no run.

use super::*;
use crate::testing::engine::FakeEngine;

#[test]
fn a_task_box_s_fresh_session_records_the_contract_and_reads_no_run_part() {
    let mut w = world(FakeDriver::new());
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(engine.launches().pop().unwrap().run.is_none());
    let events =
        w.cx.store()
            .unwrap()
            .events(BoxId::new(v.boxed.id), 200)
            .unwrap();
    let given: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "contract_given")
        .collect();
    assert_eq!(given.len(), 1, "{events:?}");
    let payload = &given.first().unwrap().payload;
    let version = moor_core::contract::contract().unwrap().version;
    assert_eq!(
        payload.get("version").and_then(serde_json::Value::as_u64),
        Some(u64::from(version))
    );
    assert_eq!(
        payload.get("harness").and_then(serde_json::Value::as_str),
        Some("claude")
    );
}
