//! A gate that passes and leaves the agent the next gate: the agent is told once, and the
//! reminder is still its own.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use moor_core::gate::{GateLine, GateNext, GateStatus};
use moor_core::workflow::{StepId, StepResult};

use super::fx::{evidence_gate, fx, gate, Fx};

fn id(name: &str) -> StepId {
    StepId::parse(name).unwrap()
}

fn lint_review_ship() -> Fx {
    fx(|_| {
        vec![
            gate("lint", &["true"], 1),
            evidence_gate("review", &["proof.md"], &[], 1),
            evidence_gate("ship", &["ship.md"], &[], 1),
        ]
    })
}

fn reminders(f: &Fx) -> usize {
    f.lines()
        .iter()
        .filter(|l| matches!(l, GateLine::Reminded { .. }))
        .count()
}

#[test]
fn the_agent_is_told_once_when_a_passed_gate_hands_it_the_next() {
    let f = lint_review_ship();
    f.quiet();
    f.pass();
    assert_eq!(
        f.standing().0,
        GateStatus::Open {
            gate: id("review"),
            try_n: 1,
            next: GateNext::Agent
        }
    );
    assert!(f.told().is_empty(), "a pass that checks does not tell");

    f.pass();
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("Gate `lint` has passed"), "{}", told[0]);
    assert!(
        told[0].contains("Gate `review` is now in hand"),
        "{}",
        told[0]
    );
    assert!(
        told[0].contains("leave proof.md in .moor/artifacts/"),
        "{}",
        told[0]
    );
    assert!(told[0].contains("step \"review\""), "{}", told[0]);
    assert_eq!(reminders(&f), 0, "the hand-over is not the reminder");
    assert_eq!(f.lines().len(), 1);

    for _ in 0..3 {
        f.pass();
    }
    assert_eq!(f.told().len(), 1, "once per gate change, not per pass");
}

#[test]
fn the_reminder_still_fires_once_after_the_hand_over() {
    let f = lint_review_ship();
    f.quiet();
    f.pass();
    f.pass();
    assert_eq!(f.told().len(), 1);

    f.quiet();
    f.pass();
    let told = f.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(
        told[1].contains("Gate `review` has no report yet"),
        "{}",
        told[1]
    );
    assert_eq!(reminders(&f), 1);
    f.pass();
    assert_eq!(f.told().len(), 2);
}

#[test]
fn each_gate_change_that_waits_for_the_agent_is_told_once() {
    let f = lint_review_ship();
    f.quiet();
    f.pass();
    f.pass();
    f.pause(1);
    f.leave(&[("proof.md", 5)]);
    let reply = f.report("review", StepResult::Ok, &["proof.md"]);
    assert!(!reply.error, "{reply:?}");

    f.quiet();
    f.pass();
    f.pass();
    let told = f.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[1].contains("Gate `review` has passed"), "{}", told[1]);
    assert!(
        told[1].contains("Gate `ship` is now in hand"),
        "{}",
        told[1]
    );
    assert_eq!(reminders(&f), 0);
}

#[test]
fn a_person_who_steered_after_the_pass_has_told_the_agent_already() {
    let f = lint_review_ship();
    f.quiet();
    f.pass();
    f.pause(1);
    f.steer("Take the review next.");
    f.quiet();
    f.pass();
    let told = f.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[1].contains("has no report yet"), "{}", told[1]);
}
