#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index what they just built"
)]

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

fn gate_no() -> &'static str {
    "The owner did not approve gate `review`.\n\nThe owner wrote: use the blue header\n\nand \
     keep it short\n\nFix it, then call report_step again with step \"review\" and its \
     evidence. moor goes through the gate again after your report."
}

#[test]
fn a_message_moor_sent_is_classed_moor_and_a_persons_stays_you() {
    let events = [
        ev(1, "steered", json!({"message": "Fix the typo."})),
        ev(
            2,
            "steered",
            json!({"message": "Gate `lint` has passed.", "origin": "moor"}),
        ),
        ev(
            3,
            "steered",
            json!({"message": "Thanks.", "origin": "person"}),
        ),
    ];
    let t = timeline(&events);
    let classes: Vec<_> = t.iter().map(|e| e.class).collect();
    assert_eq!(classes, [Class::You, Class::Moor, Class::You]);
    assert_eq!(t[1].text, "Gate `lint` has passed.");
    assert!(t.iter().all(|e| e.kind == "steered"), "{t:?}");
}

#[test]
fn the_owners_note_in_a_no_is_a_quote_apart_from_moors_words() {
    let moor = ev(
        1,
        "steered",
        json!({"message": gate_no(), "origin": "moor"}),
    );
    let t = timeline(&[moor]);
    assert_eq!(t[0].class, Class::Moor);
    assert_eq!(
        t[0].quote.as_deref(),
        Some("use the blue header\n\nand keep it short")
    );
    assert!(
        t[0].text
            .starts_with("The owner did not approve gate `review`."),
        "{}",
        t[0].text
    );
    assert!(
        t[0].text.contains("Fix it, then call report_step again"),
        "{}",
        t[0].text
    );
    assert!(!t[0].text.contains("blue header"), "{}", t[0].text);

    let typed = ev(2, "steered", json!({"message": gate_no()}));
    let t = timeline(&[typed]);
    assert_eq!(
        (t[0].class, t[0].quote.clone()),
        (Class::You, None),
        "a person's own words are theirs"
    );
}

#[test]
fn a_moor_message_typed_into_the_agent_shows_once_not_again_as_the_person() {
    use crate::progress::transcript::{Line, What};

    let at = 1_000;
    let events = [(
        at,
        ev(
            1,
            "steered",
            json!({"message": "Gate `lint` has passed.", "origin": "moor"}),
        ),
    )];
    let typed = [Line {
        id: "u1".into(),
        at: "2026-11-19T10:00:01Z".into(),
        ms: at + 1,
        what: What::Typed {
            text: "Gate `lint` has passed.".into(),
        },
    }];
    let t = timeline_with(
        &events,
        Transcript {
            lines: &typed,
            ..Transcript::default()
        },
    );
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0].class, Class::Moor);
}

#[test]
fn a_receipt_leaves_out_a_zero_count() {
    let events = [ev(1, "turn_changes", changes(3, 1, 5, 0))];
    let [only] = timeline(&events).try_into().unwrap();
    assert_eq!(only.text, "turn 3 · 2 files +10");
}
