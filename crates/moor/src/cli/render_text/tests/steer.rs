//! What `steer` prints when the agent has the text, when it waits, and when it may have it.

use moor_core::Response;
use serde_json::json;

use crate::cli::render_text::render;

#[test]
fn a_queued_steer_prints_queued_with_its_cause() {
    let v = json!({"box": "t1", "delivery": "queued", "cause": "waiting_for_room"});
    let (out, _) = render(&Response::ok("steer", Some(v)), false);
    assert_eq!(out, "queued for t1 (waiting for room)\n");
}

#[test]
fn a_steer_in_doubt_says_not_to_send_it_again() {
    let v = json!({"box": "t1", "delivery": "in_doubt"});
    let (out, _) = render(&Response::ok("steer", Some(v)), false);
    assert_eq!(out, "in doubt for t1: do not resend\n");
}
