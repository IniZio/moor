use serde_json::json;

use super::{label, parse_list, parse_one, Activity, Phase};

fn row(lifecycle: &str, session: &str) -> serde_json::Value {
    json!({"name": "fix", "repo": "oursky/moor", "branch": "moor/fix",
           "lifecycle": lifecycle, "session_state": session, "agent": {"launched": true}})
}

fn with(mut row: serde_json::Value, key: &str, value: serde_json::Value) -> serde_json::Value {
    if let Some(fields) = row.as_object_mut() {
        fields.insert(key.to_owned(), value);
    }
    row
}

#[test]
fn each_lifecycle_reads_as_the_phase_the_sidebar_shows() {
    let asking = with(
        row("ready", "idle"),
        "agent",
        json!({"launched": true, "question": "which?"}),
    );
    let waiting = with(
        row("ready", "working"),
        "held_machine",
        json!({"state": "waiting"}),
    );
    let boxes = vec![
        row("ready", "working"),
        row("ready", "idle"),
        asking,
        waiting,
        row("asleep", "idle"),
        row("closed", "ended"),
    ];
    let phases: Vec<Phase> = parse_list(json!({ "boxes": boxes }))
        .unwrap()
        .into_iter()
        .map(|b| b.phase)
        .collect();
    assert_eq!(
        phases,
        [
            Phase::Live(Activity::Working),
            Phase::Live(Activity::Idle),
            Phase::Live(Activity::Waiting),
            Phase::WaitingForMachine,
            Phase::Asleep,
            Phase::Closed,
        ]
    );
}

#[test]
fn one_box_comes_from_the_box_field() {
    let one = parse_one(json!({ "box": row("ready", "idle") }))
        .unwrap()
        .unwrap();
    assert_eq!(one.branch.as_deref(), Some("moor/fix"));
    assert_eq!(one.label(), "◈ moor/fix");
}

#[test]
fn a_label_drops_the_repo_owner_and_stands_alone_without_a_repo() {
    assert_eq!(label(Some("oursky/moor"), "fix"), "◈ moor/fix");
    assert_eq!(label(None, "fix"), "◈ fix");
}

#[test]
fn waking_a_box_sends_the_core_wake_request() {
    let wire = super::wake_wire("fix");
    let parsed = super::request(wire).unwrap();
    assert!(matches!(parsed, moor_core::request::Request::Wake(_)));
}

#[test]
fn a_box_still_setting_up_is_starting_and_shows_the_ellipsis() {
    for lifecycle in ["checking", "waiting_for_box", "setting_up"] {
        let one = parse_one(json!({ "box": row(lifecycle, "idle") }))
            .unwrap()
            .unwrap();
        assert_eq!(one.phase, Phase::Starting, "{lifecycle}");
    }
    for agent in [json!(null), json!({"launched": false})] {
        let ready = with(row("ready", "working"), "agent", agent);
        let one = parse_one(json!({ "box": ready })).unwrap().unwrap();
        assert_eq!(
            one.phase,
            Phase::Starting,
            "a ready Box whose agent isn't up yet"
        );
    }
    assert_eq!(Phase::Starting.icon(), "…");
    assert_eq!(Phase::Starting.agent_state(), "unknown");
}
