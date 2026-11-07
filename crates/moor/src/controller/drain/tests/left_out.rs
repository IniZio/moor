//! What the drain tells and leaves out: the flag it sets, and the requests it does not wait for.

use std::sync::Arc;
use std::time::Duration;

use moor_core::Request;

use super::Drain;

#[test]
fn the_flag_set_with_the_drain_turns_on_when_it_begins() {
    let flag = crate::env::Cancel::default();
    let drain = Drain::default();
    drain.set_with_drain(flag.clone());
    assert!(!flag.is_cancelled());
    drain.begin();
    assert!(flag.is_cancelled());
}
#[test]
fn a_request_left_out_of_the_drain_does_not_hold_it_until_it_ends() {
    let drain = Arc::new(Drain::default());
    let held = drain.hold().unwrap();
    let left_out = held.left_out_of_drain();
    drain.begin();
    assert_eq!(drain.settle(Duration::ZERO), 0);
    drop(left_out);
    assert_eq!(drain.settle(Duration::ZERO), 1);
}

#[test]
fn only_start_and_retry_outlive_the_drain() {
    let target = moor_core::BoxName::parse_allow_bare("demo").unwrap();
    let retry = Request::Retry(moor_core::request::BoxReq { box_name: target });
    assert!(crate::controller::progress::outlives_drain(&retry));
    assert!(!crate::controller::progress::outlives_drain(
        &Request::Show(moor_core::request::ShowReq {
            target: None,
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
        })
    ));
}
