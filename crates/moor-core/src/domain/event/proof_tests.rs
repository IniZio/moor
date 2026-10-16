//! Tests for the Box proof file event payload.

use super::*;

fn s(v: &str) -> String {
    v.to_owned()
}

/// A proof event serializes with kind "proof" and omits `refused` when it is None.
#[test]
fn proof_event_round_trips_and_omits_empty_refusal() {
    let ok = Event::Proof {
        name: s("a.png"),
        size: 3,
        sha256: s("ab"),
        refused: None,
    };
    assert_eq!(ok.kind(), "proof");
    assert_eq!(
        ok.payload(),
        serde_json::json!({"name": "a.png", "size": 3, "sha256": "ab"})
    );
    let refused = Event::Proof {
        name: s("big.bin"),
        size: 9,
        sha256: s("cd"),
        refused: Some(s("over the cap")),
    };
    assert_eq!(
        serde_json::to_value(&refused).unwrap(),
        serde_json::json!({"kind": "proof", "name": "big.bin", "size": 9, "sha256": "cd", "refused": "over the cap"})
    );
}
