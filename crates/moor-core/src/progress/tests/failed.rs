use super::*;

const AT: &str = "2026-11-18T09:59:00Z";

fn closed(events: &[(&str, Value)]) -> ProgressView {
    let b = row(BoxLifecycle::Closed, SessionState::Ended);
    let events: Vec<EventView> = events
        .iter()
        .zip(1..)
        .map(|((kind, payload), id)| ev(id, kind, AT, payload.clone()))
        .collect();
    derive(&b, &events, ten())
}

#[test]
fn egress_refusals_and_tool_errors_fail_nothing() {
    let v = closed(&[
        ("egress_refused", json!({"host": "example.com"})),
        ("proxy_credential_rejected", json!({})),
        ("close_refused", json!({})),
        ("woke", json!({})),
        ("closed", json!({})),
    ]);
    assert!(!v.failed);
    assert_eq!(v.line, "closed");
}

#[test]
fn a_closed_box_fails_for_setup_a_dead_agent_or_a_failed_run() {
    let dead = json!({"cause": "exited", "exit": 2, "session": 1});
    for (kind, payload) in [
        ("setup_failed", json!({"line": "bun install"})),
        (
            "session_failed",
            json!({"step": "launch", "message": "no claude"}),
        ),
        ("relaunch_capped", json!({})),
        ("session_ended", dead),
        ("run_failed", json!({"reason": "timed out"})),
    ] {
        let v = closed(&[(kind, payload), ("closed", json!({}))]);
        assert!(v.failed, "{kind}");
        assert_ne!(v.line, "closed", "{kind}");
    }
}

#[test]
fn a_session_that_ended_on_request_or_cleanly_is_not_a_failure() {
    for payload in [
        json!({"cause": "requested", "exit": 137, "session": 1}),
        json!({"cause": "exited", "exit": 0, "session": 1}),
        json!({"cause": "relaunch", "session": 1}),
    ] {
        assert!(!closed(&[("session_ended", payload), ("closed", json!({}))]).failed);
    }
}

#[test]
fn a_retried_setup_or_a_relaunched_agent_clears_the_earlier_failure() {
    assert!(
        !closed(&[
            ("setup_failed", json!({})),
            ("setup_finished", json!({"ok": true})),
            ("closed", json!({})),
        ])
        .failed
    );
    assert!(
        !closed(&[
            ("session_failed", json!({})),
            ("session_started", json!({"session": "s"})),
            ("closed", json!({})),
        ])
        .failed
    );
    assert!(
        closed(&[
            ("setup_failed", json!({})),
            ("setup_finished", json!({"ok": false})),
            ("closed", json!({})),
        ])
        .failed
    );
}

#[test]
fn an_open_box_fails_by_its_row_and_not_by_old_events() {
    let b = row(BoxLifecycle::SetupFailed, SessionState::Starting);
    assert!(derive(&b, &[], ten()).failed);
    let b = row(BoxLifecycle::Ready, SessionState::Idle);
    let e = [ev(1, "session_failed", AT, json!({}))];
    assert!(!derive(&b, &e, ten()).failed);
}
