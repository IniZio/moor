//! A workflow with gates: its Box starts with them, and its run follows where they stand.
#![allow(
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "a test names what it found when it fails, and walks one scenario step by step"
)]

use std::sync::Arc;

use moor_core::gate::{Approval, GateDoc, Gates};
use moor_core::workflow::{
    TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName, WorkflowSchedule,
};

use super::{repo, rig, Rig, FIRST_DUE};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::engine::FakeEngine;

const PROMPT: &str = "collect the evidence for the pull request";

/// The gate `proof`: the agent leaves `pr-*` and reports it, once.
pub(super) fn proof() -> GateDoc {
    GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["pr-*".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    }
}

/// The gate `ship`: the owner says yes, once.
pub(super) fn ship() -> GateDoc {
    GateDoc {
        name: "ship".to_owned(),
        evidence: Vec::new(),
        checks: Vec::new(),
        approval: Some(Approval::Owner),
        max_tries: None,
    }
}

impl Rig {
    /// Stores the daily workflow `evidence` with `gates`, owned by the test requester.
    pub(super) fn put_gated(&self, gates: &[GateDoc]) -> WorkflowId {
        let mut def = WorkflowDef::new(
            WorkflowName::parse("evidence").unwrap(),
            Some(WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap()),
            vec![repo()],
            PROMPT,
            (true, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.gates = Some(Gates::parse(gates).unwrap());
        self.cx
            .store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    }
}

#[test]
fn a_gated_run_starts_its_box_with_the_gates_and_tells_the_agent_no_steps() {
    let engine = Arc::new(FakeEngine::new());
    let mut r = rig();
    r.cx = r.cx.with_engine(engine.clone());
    r.answer_repo();
    let id = r.put_gated(&[proof(), ship()]);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());

    let box_id = r.latest_box(id);
    let store = r.cx.store().unwrap();
    let gates = store.box_gates(box_id).unwrap().unwrap();
    assert_eq!(gates.docs(), [proof(), ship()], "frozen into the Box");

    let launch = engine.launches().pop().expect("an agent was launched");
    let first = launch.prompt.text().unwrap_or_default().to_owned();
    assert!(first.starts_with(PROMPT), "{first}");
    assert!(
        first.contains("moor keeps this Box open until its gates pass"),
        "{first}"
    );
    assert!(
        first.contains("- Gate `proof`: leave pr-* in .moor/artifacts/"),
        "{first}"
    );
    assert!(first.contains("- Gate `ship`"), "{first}");
    for step in ["plan of", "Current step", "Done when"] {
        assert!(
            !first.contains(step),
            "no step brief, but found {step:?}: {first}"
        );
    }
}

#[test]
fn a_workflow_whose_gates_need_reports_is_refused_before_any_box_when_the_controller_cannot_take_them(
) {
    let r = rig_without_reports();
    r.answer_repo();
    let id = r.put_gated(&[proof()]);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let run = r.latest(id);
    let moor_core::workflow::RunState::Done(moor_core::workflow::RunEnd::Refused { detail }) =
        run.state
    else {
        panic!("{:?}", run.state);
    };
    assert!(detail.contains("report_step"), "{detail}");
    assert_eq!(r.open_boxes(), 0);
}

/// The rig over a controller that gives its Boxes no moor MCP server.
fn rig_without_reports() -> Rig {
    let mut r = rig();
    r.cx =
        r.cx.with_env(|e| e.with_front_step_reports(std::collections::BTreeSet::new()));
    r
}
