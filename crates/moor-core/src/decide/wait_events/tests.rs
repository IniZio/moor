//! Tests for the wait-events verdict.

use std::time::Duration;

use super::{wait_events_verdict, WaitEventsVerdict};
use crate::name::BoxName;
use crate::request::{WaitEventsReq, WaitMode};

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn names(list: &[&str]) -> Vec<BoxName> {
    list.iter()
        .map(|n| serde_json::from_str(&format!("\"{n}\"")).unwrap())
        .collect()
}

#[test]
fn any_matches_with_one_box() {
    let v = wait_events_verdict(
        WaitMode::Any,
        &names(&["a", "b"]),
        &names(&["b"]),
        secs(1),
        secs(10),
    );
    assert_eq!(v, WaitEventsVerdict::Matched);
}

#[test]
fn all_needs_every_box() {
    let wanted = names(&["a", "b"]);
    let v = wait_events_verdict(WaitMode::All, &wanted, &names(&["a"]), secs(1), secs(10));
    assert_eq!(v, WaitEventsVerdict::Keep);
    let v = wait_events_verdict(
        WaitMode::All,
        &wanted,
        &names(&["b", "a", "a"]),
        secs(1),
        secs(10),
    );
    assert_eq!(v, WaitEventsVerdict::Matched);
}

#[test]
fn match_beats_timeout() {
    let v = wait_events_verdict(
        WaitMode::Any,
        &names(&["a"]),
        &names(&["a"]),
        secs(20),
        secs(10),
    );
    assert_eq!(v, WaitEventsVerdict::Matched);
}

#[test]
fn timeout_boundary() {
    let wanted = names(&["a"]);
    let at = wait_events_verdict(WaitMode::Any, &wanted, &[], secs(10), secs(10));
    assert_eq!(at, WaitEventsVerdict::TimedOut);
    let before = wait_events_verdict(
        WaitMode::Any,
        &wanted,
        &[],
        Duration::from_millis(9999),
        secs(10),
    );
    assert_eq!(before, WaitEventsVerdict::Keep);
}

#[test]
fn empty_wanted_never_matches() {
    for mode in [WaitMode::Any, WaitMode::All] {
        let v = wait_events_verdict(mode, &[], &names(&["a"]), secs(1), secs(10));
        assert_eq!(v, WaitEventsVerdict::Keep);
    }
}

#[test]
fn req_defaults_round_trip() {
    let req: WaitEventsReq = serde_json::from_str(r#"{"boxes":["a"]}"#).unwrap();
    assert_eq!(req.boxes, names(&["a"]));
    assert_eq!(req.mode, WaitMode::Any);
    assert_eq!(req.kinds, None);
    assert_eq!(req.since, None);
    assert_eq!(req.timeout_secs, None);
    assert_eq!(
        serde_json::to_string(&req).unwrap(),
        r#"{"boxes":["a"],"mode":"any"}"#
    );
}

#[test]
fn req_parses_all_mode() {
    let req: WaitEventsReq =
        serde_json::from_str(r#"{"boxes":["a"],"mode":"all","kinds":["turn_done"],"since":3}"#)
            .unwrap();
    assert_eq!(req.mode, WaitMode::All);
    assert_eq!(req.kinds, Some(vec!["turn_done".to_owned()]));
    assert_eq!(req.since, Some(3));
    let back: WaitEventsReq = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
    assert_eq!(back, req);
}

#[test]
fn req_rejects_unknown_field() {
    assert!(serde_json::from_str::<WaitEventsReq>(r#"{"boxes":["a"],"bogus":1}"#).is_err());
}
