use moor_core::domain::event::close_reason::CloseReason;
use moor_core::user::Identity;
use serde_json::json;

use super::boxwork::fx_setup_failed;
use super::{req, requested, run_with_reason, Fx};

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
    run_with_reason(&f.cx, &req(false), CloseReason::CloseFailed).unwrap();
    assert_eq!(
        closed_payload(&f),
        json!({"from": "setup_failed", "reason": "close_failed"})
    );
}

#[test]
fn a_close_a_person_asks_for_names_them_on_the_closed_event() {
    let mut f = fx_setup_failed();
    f.cx = f.cx.with_requester(Identity::parse("github:ada").unwrap());
    requested(&f.cx, &req(false)).unwrap();
    assert_eq!(
        closed_payload(&f),
        json!({"from": "setup_failed", "reason": "requested", "by": "github:ada"})
    );
}
