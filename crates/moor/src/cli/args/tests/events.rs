//! Tests for the `events` and `wait-events` verbs.

use moor_core::controller::WatchFilter;
use moor_core::{Request, WaitEventsReq, WaitMode};

use super::{n, req, run, Action};

#[test]
fn events_defaults_to_everything_from_zero() {
    assert!(matches!(
        run(&["events"]),
        Ok((_, Action::Events { since: 0, filter, follow: false })) if filter == WatchFilter::default()
    ));
}

#[test]
fn events_takes_since_boxes_kinds_and_follow() {
    let (_, action) = run(&[
        "events",
        "--since",
        "5",
        "--box",
        "a",
        "--box",
        "b",
        "--kind",
        "turn_done",
        "--follow",
    ])
    .expect("parses");
    let want = WatchFilter {
        boxes: Some(vec![n("a"), n("b")]),
        kinds: Some(vec!["turn_done".to_string()]),
    };
    assert!(matches!(
        action,
        Action::Events { since: 5, filter, follow: true } if filter == want
    ));
}

#[test]
fn events_rejects_an_invalid_box_name() {
    assert!(run(&["events", "--box", "bad name!"]).is_err());
}

#[test]
fn wait_events_parses_all_flags() {
    assert_eq!(
        req(&[
            "wait-events",
            "a",
            "b",
            "--kind",
            "turn_done",
            "--all",
            "--since",
            "7",
            "--timeout",
            "30"
        ]),
        Request::WaitEvents(WaitEventsReq {
            boxes: vec![n("a"), n("b")],
            kinds: Some(vec!["turn_done".into()]),
            mode: WaitMode::All,
            since: Some(7),
            timeout_secs: Some(30),
        })
    );
}

#[test]
fn wait_events_defaults_and_needs_a_box() {
    assert_eq!(
        req(&["wait-events", "a"]),
        Request::WaitEvents(WaitEventsReq {
            boxes: vec![n("a")],
            kinds: None,
            mode: WaitMode::Any,
            since: None,
            timeout_secs: None,
        })
    );
    assert!(run(&["wait-events"]).is_err());
}
