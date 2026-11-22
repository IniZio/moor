//! A Box with gates is never done while one is open.

use super::*;
use crate::gate::{GateNext, GateStatus};
use crate::view::gate::GateView;
use crate::workflow::StepId;

fn gated(lifecycle: BoxLifecycle, session: SessionState, status: GateStatus) -> BoxView {
    let mut b = row(lifecycle, session);
    b.gate = Some(GateView {
        gates: vec!["proof".to_owned()],
        status,
    });
    b
}

#[test]
fn a_gated_box_first_message_shows_the_persons_words_without_the_preamble() {
    let gates = crate::gate::Gates::parse(&[crate::gate::GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["proof.md".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    }])
    .unwrap();
    let start = gates.first_message("Fix the typo.");
    let events = [
        ev(1, "steered", "t", json!({"message": start})),
        ev(2, "steered", "t", json!({"message": "keep going"})),
    ];
    let [first, second] = timeline(&events).try_into().unwrap();
    assert_eq!(first.text, "Fix the typo.");
    assert!(!first.text.contains("report_step"));
    assert_eq!(second.text, "keep going");
}

fn proof() -> StepId {
    StepId::parse("proof").unwrap()
}

fn open(next: GateNext) -> GateStatus {
    GateStatus::Open {
        gate: proof(),
        try_n: 1,
        next,
    }
}

fn stuck() -> GateStatus {
    GateStatus::Stuck {
        gate: proof(),
        why: "the agent went quiet".to_owned(),
    }
}

#[test]
fn open_gate_is_never_done() {
    for session in [SessionState::Idle, SessionState::Ended] {
        let none = derive(&row(BoxLifecycle::Ready, session), &[], ten());
        assert_eq!(none.state, Progress::Done, "no gates: {session:?}");
        for next in [GateNext::Agent, GateNext::RunChecks] {
            let b = gated(BoxLifecycle::Ready, session, open(next));
            assert_eq!(derive(&b, &[], ten()).state, Progress::Working, "{next:?}");
        }
        let passed = gated(BoxLifecycle::Ready, session, GateStatus::Passed);
        assert_eq!(derive(&passed, &[], ten()).state, Progress::Done);
    }
}

#[test]
fn an_idle_box_says_which_gate_it_waits_on() {
    let b = gated(
        BoxLifecycle::Ready,
        SessionState::Idle,
        open(GateNext::Agent),
    );
    let p = derive(&b, &[], ten());
    assert_eq!(p.line, "gate proof: waiting for the agent's evidence");
    let b = gated(
        BoxLifecycle::Ready,
        SessionState::Idle,
        open(GateNext::RunChecks),
    );
    assert_eq!(
        derive(&b, &[], ten()).line,
        "gate proof: running its checks"
    );
}

#[test]
fn a_working_agent_keeps_its_own_line_while_a_gate_is_open() {
    let b = gated(
        BoxLifecycle::Ready,
        SessionState::Working,
        open(GateNext::Agent),
    );
    let p = derive(&b, &[], ten());
    assert_eq!((p.state, p.line.as_str()), (Progress::Working, "working"));
}

#[test]
fn a_gate_that_waits_for_a_person_needs_you_even_when_the_box_sleeps() {
    for lifecycle in [BoxLifecycle::Ready, BoxLifecycle::Asleep] {
        let ask = gated(lifecycle, SessionState::Idle, open(GateNext::Ask));
        let p = derive(&ask, &[], ten());
        assert_eq!(
            (p.state, p.line.as_str()),
            (Progress::NeedsYou, "gate proof: waiting for your approval")
        );
        let stuck = gated(lifecycle, SessionState::Idle, stuck());
        let p = derive(&stuck, &[], ten());
        assert_eq!(
            (p.state, p.line.as_str()),
            (
                Progress::NeedsYou,
                "gate proof is stuck: the agent went quiet"
            )
        );
    }
}

#[test]
fn a_sleeping_box_with_a_gate_that_waits_on_the_agent_stays_asleep() {
    let b = gated(
        BoxLifecycle::Asleep,
        SessionState::Idle,
        open(GateNext::Agent),
    );
    assert_eq!(derive(&b, &[], ten()).state, Progress::Asleep);
}

#[test]
fn a_failed_gate_fails_the_box_and_says_why() {
    let failed = GateStatus::Failed {
        gate: proof(),
        why: "the owner said no".to_owned(),
    };
    let b = gated(BoxLifecycle::Ready, SessionState::Idle, failed);
    let p = derive(&b, &[], ten());
    assert_eq!(
        (p.state, p.failed, p.line.as_str()),
        (
            Progress::Failed,
            true,
            "gate proof failed: the owner said no"
        )
    );
}

#[test]
fn a_gate_never_revives_a_box_that_is_closed_or_failed() {
    for (lifecycle, want) in [
        (BoxLifecycle::Closed, Progress::Closed),
        (BoxLifecycle::SetupFailed, Progress::Failed),
    ] {
        let b = gated(lifecycle, SessionState::Idle, stuck());
        assert_eq!(derive(&b, &[], ten()).state, want);
    }
}
