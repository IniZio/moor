//! A quiet agent that has handed nothing in is reminded once, and stuck when quiet again.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use moor_core::gate::{GateLine, GateNext, GateStatus};
use moor_core::progress::Progress;
use moor_core::workflow::{StepId, StepResult};

use super::fx::{done, evidence_gate, fx};

fn id(name: &str) -> StepId {
    StepId::parse(name).unwrap()
}

#[test]
fn a_quiet_agent_is_reminded_once_and_stuck_when_quiet_again() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md", "shots-*"], &[], 1)]);
    f.pause(60);
    assert!(f.pass().is_empty());
    assert!(
        f.told().is_empty(),
        "an agent that is working is left alone"
    );

    f.quiet();
    assert!(f.pass().is_empty());
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("Gate `proof`"), "{}", told[0]);
    assert!(told[0].contains("proof.md, shots-*"), "{}", told[0]);
    assert!(matches!(
        f.lines().as_slice(),
        [GateLine::Reminded { try_n: 1, .. }]
    ));
    assert!(f.pass().is_empty());
    assert_eq!(f.told().len(), 1, "one reminder, not one per pass");
    assert_eq!(f.lines().len(), 1);

    f.quiet();
    assert!(f.pass().is_empty());
    let lines = f.lines();
    assert!(
        matches!(lines.last(), Some(GateLine::Quiet { try_n: 1, .. })),
        "{lines:?}"
    );
    assert_eq!(f.told().len(), 1, "a stuck gate is for a person");
    let (status, progress) = f.standing();
    assert!(matches!(status, GateStatus::Stuck { .. }), "{status:?}");
    assert_eq!(progress, Progress::NeedsYou);
    f.pass();
    assert_eq!(f.lines().len(), 2);
}

#[test]
fn a_report_after_the_reminder_ends_the_waiting() {
    let f = fx(|_| vec![evidence_gate("proof", &["proof.md"], &[], 1)]);
    f.quiet();
    f.pass();
    f.leave(&[("proof.md", 5)]);
    let told = f.report("proof", StepResult::Ok, &["proof.md"]);
    assert!(!told.error, "{told:?}");
    f.quiet();
    f.pass();
    assert_eq!(f.standing().0, done());
    assert_eq!(f.told().len(), 1);
}

#[test]
fn a_new_try_is_reminded_again() {
    let f = fx(|dir| {
        let check = format!("test -e {}", dir.join("fixed").display());
        vec![evidence_gate("proof", &["proof.md"], &[&check], 2)]
    });
    f.quiet();
    f.pass();
    f.leave(&[("proof.md", 5)]);
    f.report("proof", StepResult::Ok, &["proof.md"]);
    f.quiet();
    f.pass();
    assert_eq!(
        f.standing().0,
        GateStatus::Open {
            gate: id("proof"),
            try_n: 2,
            next: GateNext::Agent
        }
    );
    f.quiet();
    f.pass();
    let reminders = f
        .lines()
        .iter()
        .filter(|l| matches!(l, GateLine::Reminded { try_n: 2, .. }))
        .count();
    assert_eq!(reminders, 1, "{:?}", f.lines());
}
