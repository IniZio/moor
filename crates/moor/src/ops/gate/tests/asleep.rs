//! An agent put to sleep before it handed in a gate's evidence: the sleep ends its turn, so the
//! gate pass tells it what it waits for, which wakes the Box, and a quiet agent after that is
//! stuck for a person, asleep or not.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::gate::{GateLine, GateNext, GateStatus};
use moor_core::progress::Progress;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{StepId, StepResult};
use moor_core::{BoxName, BoxReq};

use super::fx::{evidence_gate, fx, Fx};

fn id(name: &str) -> StepId {
    StepId::parse(name).unwrap()
}

/// A person stops the Box from the portal while its agent still works.
fn stop(f: &Fx) {
    let req = BoxReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
    };
    crate::ops::sleep::run(&f.cx, &req).unwrap();
    assert_eq!(f.shown().lifecycle.as_str(), "asleep");
}

/// The inbox pass runs over an agent that comes back idle after the wake.
fn inbox_pass(f: &Fx) {
    let idle = crate::testing::agent::idle(Ts::from_millis(i64::MAX / 2));
    f.engine.set_probes("agent-1", idle);
    let backoff = crate::ops::InboxBackoff::default();
    crate::ops::inbox_pass(&f.cx, f.cx.store().unwrap(), &|| false, &backoff).unwrap();
}

fn woken(f: &Fx) -> bool {
    f.driver.calls().iter().any(|c| c.starts_with("wake "))
}

/// The agent, woken by a reminder, goes quiet again without a report: the gate is stuck and the
/// owner is told, and a Box put to sleep after that still needs them.
fn stuck_when_quiet_again(f: &Fx, gate: &str) {
    f.quiet();
    f.pass();
    let lines = f.lines();
    assert!(
        matches!(lines.last(), Some(GateLine::Quiet { gate: g, try_n: 1 }) if g == &id(gate)),
        "{lines:?}"
    );
    let (status, progress) = f.standing();
    assert!(matches!(status, GateStatus::Stuck { .. }), "{status:?}");
    assert_eq!(progress, Progress::NeedsYou);
    stop(f);
    let (status, progress) = f.standing();
    assert!(matches!(status, GateStatus::Stuck { .. }), "{status:?}");
    assert_eq!(
        progress,
        Progress::NeedsYou,
        "stuck needs a person, asleep or not"
    );
}

#[test]
fn a_gate_passed_while_the_agent_worked_is_handed_over_after_a_stop_then_reminded_then_stuck() {
    let f = fx(|_| {
        vec![
            evidence_gate("live", &["live.md"], &["true"], 1),
            evidence_gate("report", &["result.md"], &[], 1),
        ]
    });
    f.steer("Run the live tests, then write the report.");
    f.pause(60);
    f.leave(&[("live.md", 5)]);
    let reply = f.report("live", StepResult::Ok, &["live.md"]);
    assert!(!reply.error, "{reply:?}");
    f.pass();
    let open = GateStatus::Open {
        gate: id("report"),
        try_n: 1,
        next: GateNext::Agent,
    };
    assert_eq!(f.standing().0, open);
    assert!(
        f.told().len() == 1,
        "a working agent is not told: {:?}",
        f.told()
    );

    f.pause(150);
    stop(&f);
    f.pass();
    assert_eq!(
        f.told().len(),
        1,
        "the Box sleeps, so the hand-over is queued"
    );
    inbox_pass(&f);
    assert!(woken(&f), "{:?}", f.driver.calls());
    let told = f.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[1].contains("Gate `live` has passed"), "{}", told[1]);
    assert!(
        told[1].contains("Gate `report` is now in hand"),
        "{}",
        told[1]
    );

    f.quiet();
    f.pass();
    let told = f.told();
    assert_eq!(told.len(), 3, "{told:?}");
    assert!(
        told[2].contains("Gate `report` has no report yet"),
        "{}",
        told[2]
    );
    assert!(
        matches!(f.lines().last(), Some(GateLine::Reminded { try_n: 1, .. })),
        "{:?}",
        f.lines()
    );
    stuck_when_quiet_again(&f, "report");
    assert_eq!(f.told().len(), 3, "a stuck gate is for a person");
}

#[test]
fn an_agent_stopped_before_it_reported_is_reminded_then_stuck() {
    let f = fx(|_| vec![evidence_gate("report", &["result.md"], &[], 1)]);
    f.steer("Write the report.");
    f.pause(60);
    stop(&f);
    f.pass();
    assert!(
        f.told().len() == 1,
        "the Box sleeps, so the reminder is queued"
    );
    assert!(
        matches!(f.lines().as_slice(), [GateLine::Reminded { try_n: 1, .. }]),
        "{:?}",
        f.lines()
    );
    f.pass();
    assert_eq!(f.lines().len(), 1, "one reminder, not one per pass");

    inbox_pass(&f);
    assert!(woken(&f), "{:?}", f.driver.calls());
    let told = f.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(
        told[1].contains("Gate `report` has no report yet"),
        "{}",
        told[1]
    );
    stuck_when_quiet_again(&f, "report");
}

#[test]
fn a_box_whose_places_died_is_sent_nothing() {
    let f = fx(|_| vec![evidence_gate("report", &["result.md"], &[], 1)]);
    f.steer("Write the report.");
    f.pause(60);
    stop(&f);
    let capped = ExtraEvent::DeathCapped {
        reason: "oom".to_owned(),
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).event(capped))
        .unwrap();
    f.pass();
    inbox_pass(&f);
    assert!(f.lines().is_empty(), "{:?}", f.lines());
    assert_eq!(f.told().len(), 1, "only a person resumes it");
    assert!(!woken(&f));
}

#[test]
fn a_check_only_gate_is_not_judged_on_a_cut_turn() {
    let f = fx(|_| {
        vec![
            super::fx::gate("lint", &["true"], 1),
            evidence_gate("report", &["result.md"], &[], 1),
        ]
    });
    f.steer("Fix the lint.");
    f.pause(60);
    stop(&f);
    f.pass();
    assert!(f.exits().is_empty(), "{:?}", f.lines());
}

#[test]
fn an_anytime_outcome_is_not_tried_on_a_cut_turn_and_the_agent_is_reminded() {
    let f = super::fx::fx_outcomes(|_| {
        let anytime = moor_core::gate::OutcomeDoc {
            name: "nothing to do".to_owned(),
            says: None,
            tone: None,
            checks: vec![moor_core::gate::CheckDoc::from("true")],
            anytime: true,
        };
        let done = moor_core::gate::OutcomeDoc {
            name: "done".to_owned(),
            checks: Vec::new(),
            anytime: false,
            ..anytime.clone()
        };
        (
            vec![evidence_gate("report", &["result.md"], &[], 1)],
            vec![anytime, done],
        )
    });
    f.steer("Write the report.");
    f.pause(60);
    stop(&f);
    f.pass();
    let lines = f.lines();
    assert!(
        matches!(lines.as_slice(), [GateLine::Reminded { try_n: 1, .. }]),
        "{lines:?}"
    );
}
