use super::*;
use serde_json::{json, Value};

fn at(kind: &str, payload: Value, ts: &str) -> Happened {
    Happened {
        kind: kind.to_owned(),
        payload,
        ts: ts.to_owned(),
    }
}

fn stalled(ts: &str) -> Happened {
    at(
        "agent_stalled",
        json!({"doing": "Bash cargo test", "quiet_secs": 725}),
        ts,
    )
}

#[test]
fn no_events_is_not_stuck() {
    assert_eq!(stuck_of(&[]), None);
}

#[test]
fn ordinary_events_are_not_stuck() {
    let events = [
        at("started", json!({}), "2026-01-01T00:00:00Z"),
        at("busy", json!({}), "2026-01-01T00:01:00Z"),
        at("agent_said", json!({"text": "hi"}), "2026-01-01T00:02:00Z"),
    ];
    assert_eq!(stuck_of(&events), None);
}

#[test]
fn one_egress_refusal_is_stuck_with_its_host_and_rule() {
    let events = [at(
        "egress_refused",
        json!({"host": "api.example.com", "rule": "example", "method": null}),
        "2026-01-01T00:05:00Z",
    )];
    assert_eq!(
        stuck_of(&events),
        Some(StuckView {
            kind: "egress_refused".to_owned(),
            since: "2026-01-01T00:05:00Z".to_owned(),
            detail: "refused api.example.com (rule example)".to_owned(),
        })
    );
}

#[test]
fn a_stall_reads_as_stalled_and_names_what_the_agent_was_doing() {
    let view = stuck_of(&[stalled("2026-01-01T00:20:00Z")]).expect("stuck");
    assert_eq!(view.kind, "stalled");
    assert_eq!(view.since, "2026-01-01T00:20:00Z");
    assert_eq!(
        view.detail,
        "no hook report for 12 min while Bash cargo test"
    );
}

#[test]
fn a_stall_without_doing_names_only_the_quiet_time() {
    for payload in [
        json!({"doing": null, "quiet_secs": 600}),
        json!({"quiet_secs": 600}),
    ] {
        let view =
            stuck_of(&[at("agent_stalled", payload, "2026-01-01T00:20:00Z")]).expect("stuck");
        assert_eq!(view.detail, "no hook report for 10 min");
    }
}

#[test]
fn a_resume_clears_a_stall() {
    let events = [
        stalled("2026-01-01T00:20:00Z"),
        at(
            "agent_resumed",
            json!({"quiet_secs": 900}),
            "2026-01-01T00:35:00Z",
        ),
    ];
    assert_eq!(stuck_of(&events), None);
}

#[test]
fn a_finished_turn_clears_a_stall() {
    let events = [
        stalled("2026-01-01T00:20:00Z"),
        at(
            "turn_done",
            json!({"steer": 1, "outcome": "done"}),
            "2026-01-01T00:25:00Z",
        ),
    ];
    assert_eq!(stuck_of(&events), None);
}

#[test]
fn every_clearing_kind_clears() {
    for kind in [
        "agent_resumed",
        "turn_done",
        "idled",
        "needs_input",
        "session_started",
        "session_ended",
        "closed",
        "agent_link_restored",
        "proxy_credential_recovered",
    ] {
        let events = [
            at(
                "agent_unreachable",
                json!({"attempts": 3, "error": "x"}),
                "2026-01-01T00:00:00Z",
            ),
            at(kind, json!({}), "2026-01-01T00:01:00Z"),
        ];
        assert_eq!(stuck_of(&events), None, "{kind}");
    }
}

#[test]
fn the_newest_of_two_signals_wins() {
    let events = [
        at(
            "egress_refused",
            json!({"host": "a.example"}),
            "2026-01-01T00:05:00Z",
        ),
        stalled("2026-01-01T00:30:00Z"),
    ];
    assert_eq!(stuck_of(&events).expect("stuck").kind, "stalled");
    let events = [
        stalled("2026-01-01T00:30:00Z"),
        at(
            "egress_refused",
            json!({"host": "a.example"}),
            "2026-01-01T00:40:00Z",
        ),
    ];
    let view = stuck_of(&events).expect("stuck");
    assert_eq!(view.kind, "egress_refused");
    assert_eq!(view.since, "2026-01-01T00:40:00Z");
}

#[test]
fn a_signal_after_a_clear_is_stuck_again() {
    let events = [
        stalled("2026-01-01T00:20:00Z"),
        at(
            "agent_resumed",
            json!({"quiet_secs": 900}),
            "2026-01-01T00:35:00Z",
        ),
        stalled("2026-01-01T01:00:00Z"),
    ];
    assert_eq!(
        stuck_of(&events).expect("stuck").since,
        "2026-01-01T01:00:00Z"
    );
}

#[test]
fn details_name_the_fields_each_kind_carries() {
    let detail = |kind: &str, payload: Value| {
        stuck_of(&[at(kind, payload, "2026-01-01T00:00:00Z")])
            .expect("stuck")
            .detail
    };
    assert_eq!(
        detail(
            "agent_unreachable",
            json!({"attempts": 4, "error": "timed out"})
        ),
        "unreachable after 4 tries: timed out"
    );
    assert_eq!(
        detail("agent_link_lost", json!({"pane": "%1", "exit": 137})),
        "pane %1 ended with exit 137"
    );
    assert_eq!(
        detail(
            "agent_rate_limited",
            json!({"rule": "gh", "status": 429, "resets_at": null})
        ),
        "rule gh rate limited (429)"
    );
    assert_eq!(
        detail(
            "credential_withheld",
            json!({"host": "api.github.com", "rule": "gh", "method": "GET"})
        ),
        "credential withheld for api.github.com (rule gh)"
    );
    assert_eq!(
        detail("proxy_credential_failed", json!({"rule": "gh"})),
        "credential for rule gh failed"
    );
    assert_eq!(
        detail("egress_aborted", json!({"host": "x.example", "rule": null})),
        "handshake to x.example aborted"
    );
}

#[test]
fn a_detail_falls_back_to_the_kind_in_words() {
    for (kind, words) in [
        ("agent_link_lost", "agent link lost"),
        ("agent_unreachable", "agent unreachable"),
        ("proxy_credential_failed", "proxy credential failed"),
    ] {
        let view = stuck_of(&[at(kind, json!({}), "2026-01-01T00:00:00Z")]).expect("stuck");
        assert_eq!(view.detail, words, "{kind}");
    }
}
