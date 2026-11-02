use serde_json::json;

use super::boxwork::fx_setup_failed;
use super::{req, run_with_reason, Fx};

fn closed_payload(f: &Fx) -> serde_json::Value {
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    events
        .iter()
        .find(|e| e.kind == "closed")
        .expect("a closed event")
        .payload
        .clone()
}

#[test]
fn a_close_with_a_reason_records_it_on_the_closed_event() {
    let f = fx_setup_failed();
    run_with_reason(&f.cx, &req(false), Some("close_failed".to_owned())).unwrap();
    assert_eq!(
        closed_payload(&f),
        json!({"from": "setup_failed", "reason": "close_failed"})
    );
}

#[test]
fn a_plain_close_has_no_reason_key() {
    let f = fx_setup_failed();
    super::run(&f.cx, &req(false)).unwrap();
    assert_eq!(closed_payload(&f), json!({"from": "setup_failed"}));
}
