use moor_core::domain::decision::{Decision, ExtraEvent};

use super::{new_box, open};

fn said(store: &super::super::Store, id: moor_core::BoxId, text: &str) {
    store
        .apply(Decision::new(id).event(ExtraEvent::AgentSaid { text: text.into() }))
        .unwrap();
}

#[test]
fn newest_events_of_returns_the_last_n_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = new_box(&store, "alpha");
    let b = new_box(&store, "beta");
    said(&store, a, "one");
    said(&store, b, "other box");
    said(&store, a, "two");
    said(&store, a, "three");
    let rows = store.newest_events_of(a, "agent_said", 2).unwrap();
    let texts: Vec<_> = rows
        .iter()
        .map(|r| r.payload.get("text").and_then(|t| t.as_str()).unwrap())
        .collect();
    assert_eq!(texts, ["two", "three"]);
    assert!(rows.iter().all(|r| r.box_id == a && r.kind == "agent_said"));
}

#[test]
fn newest_events_of_is_empty_without_a_match() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = new_box(&store, "alpha");
    assert!(store
        .newest_events_of(a, "agent_said", 5)
        .unwrap()
        .is_empty());
    said(&store, a, "one");
    assert!(store.newest_events_of(a, "idled", 5).unwrap().is_empty());
    assert!(store
        .newest_events_of(a, "agent_said", 0)
        .unwrap()
        .is_empty());
}
