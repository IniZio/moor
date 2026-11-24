//! A start names no task on the wire: only `box-tool` ties a Box to a place.

use super::*;

#[test]
fn a_wire_start_naming_a_task_is_refused() {
    let r = serde_json::from_str::<Request>(r#"{"cmd":"start","name":"a","task":3}"#);
    let err = r.expect_err("task is set only by box-tool").to_string();
    assert!(err.contains("unknown field `task`"), "{err}");
    let mut start = StartReq::named(n("a"));
    start.task = Some(BoxId::new(3));
    let wire = serde_json::to_string(&start).unwrap();
    assert!(!wire.contains("task"), "{wire}");
}
