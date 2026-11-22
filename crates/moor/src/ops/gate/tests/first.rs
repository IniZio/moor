//! A gate that asks the owner before anything was handed in: the owner is told once, when the
//! agent has gone quiet.

use moor_core::gate::{Approval, GateDoc, GateNext, GateStatus};
use moor_core::inbox::{InboxState, InboxValue};
use moor_core::progress::Progress;
use moor_core::request::InboxOp;
use moor_core::seam::clock::Ts;
use moor_core::user::Viewer;
use moor_core::workflow::StepId;

use super::fx::{fx, gate, Fx};
use crate::testing::agent;
use crate::testing::cx::REQUESTER_USER;

/// A Box whose only gate asks its owner, seen by its owner.
fn approval_only() -> Fx {
    let mut f = fx(|_| {
        vec![GateDoc {
            approval: Some(Approval::Owner),
            ..gate("ok", &[], 2)
        }]
    });
    f.cx = f.cx.with_viewer(Viewer::User(REQUESTER_USER));
    f.engine
        .set_probes("agent-1", agent::working(Ts::from_millis(0)));
    f
}

/// The agent finishes its turn and goes quiet.
fn quiet(f: &Fx) {
    f.engine
        .set_probes("agent-1", agent::idle(Ts::from_millis(0)));
    f.quiet();
}

fn asking() -> GateStatus {
    GateStatus::Open {
        gate: StepId::parse("ok").unwrap(),
        try_n: 1,
        next: GateNext::Ask,
    }
}

fn entries(f: &Fx) -> (u64, Vec<InboxState>) {
    let store = f.cx.store().unwrap();
    let value = crate::ops::users::inbox::run(store, REQUESTER_USER, &InboxOp::List).unwrap();
    let items = serde_json::from_value::<InboxValue>(value).unwrap().items;
    let events = store.count_events_of(f.id, "gate_waiting").unwrap();
    (events, items.into_iter().map(|i| i.state).collect())
}

#[test]
fn an_approval_only_gate_asks_nobody_and_shows_working_while_the_agent_works() {
    let f = approval_only();
    for _ in 0..3 {
        f.pass();
    }
    assert_eq!(f.standing(), (asking(), Progress::Working));
    assert_eq!(entries(&f), (0, Vec::new()));
}

#[test]
fn an_approval_only_gate_writes_one_entry_when_the_agent_goes_quiet() {
    let f = approval_only();
    f.pass();
    assert_eq!(entries(&f), (0, Vec::new()));
    quiet(&f);
    f.pass();
    assert_eq!(f.standing(), (asking(), Progress::NeedsYou));
    assert_eq!(entries(&f), (1, vec![InboxState::Unread]));
    assert!(f.shown().progress.unwrap().needs.is_some());
}

#[test]
fn a_second_pass_writes_no_second_entry_for_an_approval_only_gate() {
    let f = approval_only();
    quiet(&f);
    for _ in 0..3 {
        f.pass();
    }
    assert_eq!(entries(&f), (1, vec![InboxState::Unread]));
}
