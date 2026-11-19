use serde_json::{json, Value};

use super::*;

fn ev(id: i64, kind: &str, payload: Value) -> EventView {
    EventView {
        id,
        kind: kind.into(),
        payload,
        ts: "2026-11-19T10:00:00Z".into(),
    }
}

fn changes(turn: u64, files: u64, added: u64, removed: u64) -> Value {
    let tally = json!({"files": files, "added": added, "removed": removed});
    json!({"turn": turn, "repos": [
        {"repo": "o/a", "turn": tally, "total": tally, "files": []},
        {"repo": "o/b", "turn": tally, "total": {"files": 1, "added": 0, "removed": 0}, "files": []},
    ]})
}

#[test]
fn a_receipt_joins_the_turn_it_follows_and_sums_every_repo() {
    let events = [
        ev(1, "steered", json!({"message": "go"})),
        ev(2, "turn_done", json!({"steer": 1, "outcome": "idle"})),
        ev(3, "idled", json!({})),
        ev(4, "turn_changes", changes(2, 2, 20, 3)),
    ];
    let t = timeline(&events);
    let turn = t.iter().find(|e| e.id == 2).unwrap();
    assert_eq!(turn.text, "turn 2 · 4 files +40 −6");
    let receipt = turn.receipt.unwrap();
    assert_eq!(receipt.total.files, 3);
    assert!(t.iter().all(|e| e.id != 4), "{t:?}");
}

#[test]
fn a_receipt_with_no_turn_line_shows_on_its_own_and_a_turn_without_one_keeps_its_words() {
    let events = [
        ev(1, "turn_changes", changes(1, 0, 0, 0)),
        ev(2, "turn_done", json!({"steer": 1, "outcome": "idle"})),
    ];
    let [alone, turn] = timeline(&events).try_into().unwrap();
    assert_eq!(alone.text, "turn 1 · no changes");
    assert_eq!(alone.class, Class::Turn);
    assert_eq!(turn.text, "Turn ended (idle)");
    assert!(turn.receipt.is_none());
}
