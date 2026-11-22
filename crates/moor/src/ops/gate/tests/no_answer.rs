//! A check moor gets no answer from: the next pass runs it again, and a second in a row in the
//! same try waits for a person, with no try spent and nothing steered to the agent.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index what they just counted"
)]

use moor_core::gate::{closing, CheckDoc, GateLine, GateStatus, OutcomeDoc, Wait};
use moor_core::progress::Progress;

use super::fx::{busy, evidence_gate, fx_on, fx_outcomes_on, gate, Fx};
use crate::testing::driver::FakeDriver;

/// An outcome whose one check is `run`.
fn outcome(name: &str, run: &str, anytime: bool) -> OutcomeDoc {
    OutcomeDoc {
        name: name.to_owned(),
        says: None,
        tone: None,
        checks: vec![CheckDoc::from(run)],
        anytime,
    }
}

fn no_answers(f: &Fx) -> usize {
    f.lines()
        .iter()
        .filter(|l| matches!(l, GateLine::NoAnswer { .. }))
        .count()
}

/// The card a person sees, with the stage stuck at try 1.
fn card(f: &Fx, stage: &str) -> Wait {
    let shown = f.shown();
    let progress = shown.progress.unwrap();
    assert_eq!(progress.state, Progress::NeedsYou);
    let status = shown.gate.unwrap().status;
    assert!(
        matches!(&status, GateStatus::Stuck { gate, try_n: 1, why, .. } if gate.as_str() == stage && why.contains("not the agent")),
        "{status:?}"
    );
    progress.needs.unwrap().gate.unwrap()
}

/// Three passes after the stage got stuck run nothing and write nothing.
fn stays_put(f: &Fx) {
    let before = f.lines().len();
    for _ in 0..3 {
        f.quiet();
        assert!(f.pass().is_empty(), "a busy exec would log");
    }
    assert_eq!(f.lines().len(), before);
}

#[test]
fn a_gates_check_moor_cannot_run_twice_waits_for_a_person() {
    let f = fx_on(
        FakeDriver::new().exec_fails_when("busy-always", busy()),
        |_| vec![gate("tests", &["echo busy-always"], 2)],
    );
    f.quiet();
    assert!(!f.pass().is_empty(), "the busy Box is logged");
    assert_eq!(no_answers(&f), 1);
    assert!(matches!(f.standing().0, GateStatus::Open { .. }));
    f.pass();
    assert_eq!(no_answers(&f), 2);
    let w = card(&f, "tests");
    assert_eq!(w.command.as_deref(), Some("echo busy-always"));
    assert!(w.why.unwrap().contains("busy"));
    assert!(w.hint.contains("not the agent"), "{}", w.hint);
    assert!(
        f.told().is_empty(),
        "the agent is not steered for moor's fault"
    );
    stays_put(&f);
}

#[test]
fn a_busy_box_twice_at_the_closing_stage_waits_for_a_person() {
    let f = fx_outcomes_on(FakeDriver::new().exec_fails_when("merged", busy()), |_| {
        let outcomes = vec![
            outcome("merged", "echo merged", false),
            outcome("open", "true", false),
        ];
        (vec![gate("tests", &["true"], 2)], outcomes)
    });
    f.quiet();
    f.pass();
    f.pass();
    assert_eq!(no_answers(&f), 1);
    f.pass();
    assert_eq!(no_answers(&f), 2);
    let w = card(&f, closing().as_str());
    assert_eq!(w.command.as_deref(), Some("echo merged"));
    assert!(w.why.unwrap().contains("outcome `merged`"));
    assert!(
        f.told().is_empty(),
        "no steer, and the later outcome never won"
    );
    stays_put(&f);
}

#[test]
fn an_anytime_check_moor_cannot_run_twice_makes_the_waiting_gate_wait_for_a_person() {
    let f = fx_outcomes_on(FakeDriver::new().exec_fails_when("idle", busy()), |_| {
        let outcomes = vec![
            outcome("nothing to do", "echo idle", true),
            outcome("done", "true", false),
        ];
        (vec![evidence_gate("proof", &["pr-*"], &[], 2)], outcomes)
    });
    f.quiet();
    f.pass();
    f.pass();
    assert_eq!(no_answers(&f), 2);
    let w = card(&f, "proof");
    assert_eq!(w.command.as_deref(), Some("echo idle"));
    assert!(
        f.told().is_empty(),
        "no reminder for a check moor could not run"
    );
    stays_put(&f);
}

#[test]
fn a_person_reopens_it_and_the_count_starts_afresh() {
    let f = fx_on(
        FakeDriver::new().exec_fails_when("busy-always", busy()),
        |_| vec![gate("tests", &["echo busy-always"], 2)],
    );
    f.quiet();
    f.pass();
    f.pass();
    assert!(matches!(f.standing().0, GateStatus::Stuck { .. }));
    f.steer("look again");
    f.quiet();
    f.pass();
    assert_eq!(no_answers(&f), 3);
    assert!(
        matches!(f.standing().0, GateStatus::Open { try_n: 1, .. }),
        "{:?}",
        f.standing()
    );
    f.pass();
    assert!(matches!(f.standing().0, GateStatus::Stuck { try_n: 1, .. }));
}
