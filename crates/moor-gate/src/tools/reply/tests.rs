//! Tests for what `box_status` and `list_boxes` add to the controller's answers.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use serde_json::{json, Value};

use super::{detailed, listed};

/// A `show` of one Box whose agent said something, then `later` other events happened.
fn shown(later: usize) -> Value {
    let mut events = vec![json!({"kind": "agent_said", "payload": {"text": "PR 7"},
        "ts": "2026-10-29T10:00:00Z"})];
    events.extend((0..later).map(|n| json!({"kind": "egress", "payload": {"n": n}, "ts": "x"})));
    json!({
        "box": {"lifecycle": "ready",
            "said": {"text": "PR 7", "at": "2026-10-29T10:00:00Z"}},
        "events": events,
        "artifacts": [{"name": "shot.png"}],
    })
}

#[test]
fn the_agents_message_survives_the_event_trim() {
    let mut value = shown(9);
    detailed(&mut value);
    assert_eq!(value["events"].as_array().unwrap().len(), 5);
    assert!(value["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["kind"] != "agent_said"));
    assert_eq!(
        value["agent_said"],
        json!({"text": "PR 7", "at": "2026-10-29T10:00:00Z"})
    );
    assert_eq!(value["artifacts"], json!([{"name": "shot.png"}]));
}

/// A ready Box whose events are `kinds`, in order, each with `payload`.
fn with_events(events: &[(&str, Value)]) -> Value {
    let events: Vec<Value> = events
        .iter()
        .enumerate()
        .map(|(n, (kind, payload))| {
            json!({"kind": kind, "payload": payload, "ts": format!("2026-10-29T10:0{n}:00Z")})
        })
        .collect();
    json!({"box": {"lifecycle": "ready"}, "events": events})
}

#[test]
fn a_refused_egress_marks_the_box_stuck_in_the_status_line() {
    let mut value = with_events(&[(
        "egress_refused",
        json!({"host": "api.example.com", "rule": "example"}),
    )]);
    detailed(&mut value);
    assert_eq!(value["stuck"]["kind"], "egress_refused");
    assert_eq!(value["stuck"]["since"], "2026-10-29T10:00:00Z");
    assert_eq!(
        value["stuck"]["detail"],
        "refused api.example.com (rule example)"
    );
    assert_eq!(value["status"], "ready · stuck: egress_refused");
}

#[test]
fn a_finished_turn_after_it_clears_stuck() {
    let mut value = with_events(&[
        ("egress_refused", json!({"host": "api.example.com"})),
        ("turn_done", json!({"steer": 1, "outcome": "done"})),
    ]);
    detailed(&mut value);
    assert_eq!(value["stuck"], Value::Null);
    assert_eq!(value["status"], "ready");
}

#[test]
fn stuck_is_judged_before_the_events_are_trimmed() {
    let mut events = vec![("agent_stalled", json!({"doing": null, "quiet_secs": 700}))];
    events.extend((0..9).map(|n| ("agent_doing", json!({"text": format!("{n}")}))));
    let mut value = with_events(&events);
    detailed(&mut value);
    assert_eq!(value["events"].as_array().unwrap().len(), 5);
    assert_eq!(value["stuck"]["kind"], "stalled");
}

#[test]
fn a_waiting_stuck_box_reads_waiting_then_stuck() {
    let mut value = with_events(&[("agent_unreachable", json!({"attempts": 3, "error": "x"}))]);
    value["box"]["agent"] = json!({"state": "waiting", "waiting_on": ["CI"]});
    detailed(&mut value);
    assert_eq!(
        value["status"],
        "ready · waiting on CI · stuck: agent_unreachable"
    );
}

#[test]
fn a_box_whose_agent_said_nothing_has_no_agent_said() {
    let mut value = json!({"box": {"lifecycle": "ready"}, "events": []});
    detailed(&mut value);
    assert!(value.get("agent_said").is_none());
    assert_eq!(value["status"], "ready");
}

#[test]
fn a_waiting_agent_names_what_it_waits_on_in_detail() {
    let mut value = shown(0);
    value["box"]["agent"] = json!({"state": "waiting", "waiting_on": ["CI", "deploy"]});
    detailed(&mut value);
    assert_eq!(value["status"], "ready · waiting on CI, deploy");
    value["box"]["agent"] = json!({"state": "waiting", "waiting_on": []});
    detailed(&mut value);
    assert_eq!(value["status"], "ready · waiting on background work");
}

#[test]
fn a_waiting_agent_names_what_it_waits_on_in_the_list() {
    let mut value = json!({"boxes": [
        {"name": "r/a", "lifecycle": "ready",
         "agent": {"state": "waiting", "waiting_on": ["CI"]}},
        {"name": "r/b", "lifecycle": "ready", "agent": {"state": "idle"}},
    ]});
    listed(&mut value, &|_| Err("unused".to_owned()));
    assert_eq!(value["boxes"][0]["status"], "ready · waiting on CI");
    assert_eq!(value["boxes"][1]["status"], "ready");
}

fn cold(reason: &str) -> (&'static str, Value) {
    (
        "cache_cold",
        json!({"reason": reason, "detail": "provider x keeps no cache volumes"}),
    )
}

#[test]
fn a_box_that_started_without_cache_volumes_says_why_and_is_not_stuck() {
    let mut value = with_events(&[cold("provider_unsupported")]);
    detailed(&mut value);
    assert_eq!(value["status"], "ready");
    assert_eq!(value["stuck"], Value::Null);
    assert!(value.get("failure").is_none());
    assert_eq!(value["cache_cold"]["reason"], "provider_unsupported");
    assert_eq!(
        value["cache_cold"]["detail"],
        "provider x keeps no cache volumes"
    );
}

#[test]
fn the_cold_cache_is_judged_before_the_events_are_trimmed() {
    let mut events = vec![cold("switched_off")];
    events.extend((0..9).map(|n| ("agent_doing", json!({"text": format!("{n}")}))));
    let mut value = with_events(&events);
    detailed(&mut value);
    assert_eq!(value["events"].as_array().unwrap().len(), 5);
    assert_eq!(value["cache_cold"]["reason"], "switched_off");
}

#[test]
fn a_box_that_kept_its_cache_volumes_has_no_cold_cache() {
    let mut value = with_events(&[("agent_doing", json!({"text": "x"}))]);
    detailed(&mut value);
    assert_eq!(value["cache_cold"], Value::Null);
}
