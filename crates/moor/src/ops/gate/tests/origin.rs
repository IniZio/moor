//! Who wrote what the agent was sent: moor's gate steers say so, a person's do not.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index what they just counted"
)]

use super::fx::{evidence_gate, fx, gate, Fx};

fn lint_review() -> Fx {
    fx(|_| {
        vec![
            gate("lint", &["true"], 1),
            evidence_gate("review", &["proof.md"], &[], 1),
        ]
    })
}

/// The `origin` each `steered` event of the Box records, oldest first; none when it says nothing.
pub(super) fn origins(f: &Fx) -> Vec<Option<String>> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "steered")
        .map(|e| e.payload.get("origin")?.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn the_hand_over_and_the_reminder_are_moors_and_a_persons_steer_is_not() {
    let f = lint_review();
    f.quiet();
    f.pass();
    f.pass();
    f.quiet();
    f.pass();
    f.steer("Take the review next.");
    let told = f.told();
    assert_eq!(told.len(), 3, "{told:?}");
    assert!(told[0].contains("is now in hand"), "{}", told[0]);
    assert!(told[1].contains("has no report yet"), "{}", told[1]);
    let moor = Some("moor".to_owned());
    assert_eq!(origins(&f), [moor.clone(), moor, None]);
}
