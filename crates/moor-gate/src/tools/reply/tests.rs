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
