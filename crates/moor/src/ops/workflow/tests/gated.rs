//! A workflow with gates: its Box starts with them, and its run follows where they stand.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "a test indexes the JSON it just read, names what it found when it fails, and walks one scenario step by step"
)]

use std::sync::Arc;

use moor_core::gate::{Approval, GateDoc, GateLine, Gates};
use moor_core::workflow::{
    Answer, CancelCause, CommitSha, RunEnd, RunOutcome, RunState, StepId, StepReport, StepResult,
    TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName, WorkflowSchedule,
};

use super::hook::{cancelled, hooked, NEW, OLD};
use super::{repo, rig, Rig, FIRST_DUE};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::engine::FakeEngine;

const MIN: i64 = 60_000;

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
        self.put_gated_for(gates, false)
    }

    /// Like [`Rig::put_gated`], firing on pull requests when `on_pull_request`.
    fn put_gated_for(&self, gates: &[GateDoc], on_pull_request: bool) -> WorkflowId {
        let mut def = WorkflowDef::new(
            WorkflowName::parse("evidence").unwrap(),
            Some(WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap()),
            vec![repo()],
            PROMPT,
            (true, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.gates = Some(Gates::parse(gates).unwrap());
        def.on_pull_request = on_pull_request;
        self.cx
            .store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    }
}

impl Rig {
    /// Fires the workflow at 09:00 and starts its Box, which works at the first gate.
    fn started_gated(&self, gates: &[GateDoc]) -> WorkflowId {
        self.answer_repo();
        let id = self.put_gated(gates);
        self.advance_to(FIRST_DUE + 1_000);
        assert!(self.pass().is_empty());
        id
    }

    /// Writes `line` as the next gate line of the newest run's Box.
    fn gate_line(&self, id: WorkflowId, line: &GateLine) {
        let (box_id, store) = (self.latest_box(id), self.cx.store().unwrap());
        let n = i64::try_from(store.gate_lines(box_id).unwrap().len()).unwrap() + 1;
        store
            .append_gate_line(box_id, n, self.cx.clock().now(), line)
            .unwrap();
    }

    /// The newest event of `kind` of the newest run's Box, as its payload.
    fn event(&self, id: WorkflowId, kind: &str) -> Option<serde_json::Value> {
        let store = self.cx.store().unwrap();
        store
            .newest_event_of(self.latest_box(id), kind)
            .unwrap()
            .map(|e| e.payload)
    }

    /// The newest run's step and try while it works or waits.
    fn at(&self, id: WorkflowId) -> (&'static str, (String, u8)) {
        let run = self.latest(id);
        let phase = run.state.phase().as_str();
        (phase, super::plan::step_of(&run.state))
    }

    /// The newest run's outcome, once it ended.
    fn outcome(&self, id: WorkflowId) -> RunOutcome {
        match self.latest(id).state {
            RunState::Done(RunEnd::Ended { outcome, .. }) => outcome,
            other => panic!("not ended: {other:?}"),
        }
    }
}

fn step(name: &str) -> StepId {
    StepId::parse(name).unwrap()
}

/// The agent's accepted report of `gate` at try 1, with `note`.
fn reported(gate: &str, note: &str) -> GateLine {
    GateLine::Submitted {
        gate: step(gate),
        try_n: 1,
        report: StepReport {
            step: step(gate),
            result: StepResult::Ok,
            evidence: vec!["pr-1.md".to_owned()],
            note: note.to_owned(),
        },
    }
}

/// The agent was reminded and went quiet again, so `gate` is stuck.
fn gone_quiet(id: WorkflowId, r: &Rig, gate: &str) {
    for line in [
        GateLine::Reminded {
            gate: step(gate),
            try_n: 1,
        },
        GateLine::Quiet {
            gate: step(gate),
            try_n: 1,
        },
    ] {
        r.gate_line(id, &line);
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

#[test]
fn an_idle_agent_at_an_open_gate_leaves_the_run_running() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    r.quiet_after_work(id);
    for _ in 0..3 {
        r.pause(30 * MIN);
        assert!(r.pass().is_empty());
        assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
    }
    assert!(r.kinds(id).contains(&"entered".to_owned()));
}

#[test]
fn passed_gates_deliver_the_run_with_the_note_of_the_last_accepted_report() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    r.gate_line(id, &reported("proof", "all green"));
    r.pass();
    assert_eq!(
        r.outcome(id),
        RunOutcome::Delivered {
            result: "all green".to_owned()
        }
    );
    assert!(r.event(id, "run_failed").is_none());
}

#[test]
fn a_gate_that_failed_for_good_fails_the_run_with_its_reason() {
    let r = rig();
    let once = GateDoc {
        max_tries: Some(1),
        ..ship()
    };
    let id = r.started_gated(&[once]);
    let no = GateLine::Answered {
        gate: step("ship"),
        try_n: 1,
        by: "me".to_owned(),
        answer: Answer::No,
        note: "not like this".to_owned(),
    };
    r.gate_line(id, &no);
    r.pass();
    let RunOutcome::Failed { at, reason } = r.outcome(id) else {
        panic!("{:?}", r.latest(id).state);
    };
    assert_eq!(at.as_str(), "ship");
    assert!(
        reason.contains("the owner said no: not like this"),
        "{reason}"
    );
    let told = r.event(id, "run_failed").expect("the owner hears of it");
    assert_eq!(told["reason"], reason);
}

#[test]
fn a_stuck_gate_sets_the_run_waiting_and_a_reopened_one_goes_on_to_delivery() {
    let r = rig();
    let id = r.started_gated(&[proof()]);
    gone_quiet(id, &r, "proof");
    r.pass();
    assert_eq!(r.at(id), ("waiting", ("proof".to_owned(), 1)));
    let told = r.event(id, "run_waiting").expect("a run_waiting notice");
    assert_eq!(told["step"], "proof");
    assert!(
        told["question"]
            .as_str()
            .unwrap()
            .contains("Gate `proof` is stuck"),
        "{told}"
    );

    // Longer than the run's timeout, which does not run while it waits.
    r.pause(5 * 60 * MIN);
    r.pass();
    assert_eq!(r.at(id).0, "waiting");
    let reopened = GateLine::Reopened {
        gate: step("proof"),
        by: "me".to_owned(),
    };
    r.gate_line(id, &reopened);
    r.pass();
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
    assert!(r.kinds(id).contains(&"resumed".to_owned()));

    r.gate_line(id, &reported("proof", "fixed"));
    r.pass();
    assert_eq!(
        r.outcome(id),
        RunOutcome::Delivered {
            result: "fixed".to_owned()
        }
    );
}

#[test]
fn a_gate_that_asks_sets_the_run_waiting_and_the_owners_yes_delivers_it() {
    let r = rig();
    let id = r.started_gated(&[proof(), ship()]);
    r.gate_line(id, &reported("proof", "proof is in"));
    r.quiet_after_work(id);
    r.pause(MIN);
    r.pass();
    assert_eq!(r.at(id), ("waiting", ("ship".to_owned(), 1)));
    let told = r.event(id, "run_waiting").expect("a run_waiting notice");
    assert!(
        told["question"]
            .as_str()
            .unwrap()
            .contains("asks your approval"),
        "{told}"
    );

    let refused = r
        .answer_the_run(id, Answer::Yes)
        .unwrap_err()
        .error
        .to_string();
    assert!(refused.contains("answer_gate"), "{refused}");

    let yes = GateLine::Answered {
        gate: step("ship"),
        try_n: 1,
        by: "me".to_owned(),
        answer: Answer::Yes,
        note: String::new(),
    };
    r.gate_line(id, &yes);
    r.pass();
    assert_eq!(
        r.outcome(id),
        RunOutcome::Delivered {
            result: "proof is in".to_owned()
        }
    );
}

#[test]
fn a_newer_head_cancels_a_gated_pull_request_run_as_it_does_any_other() {
    let r = hooked();
    let id = r.put_gated_for(&[proof()], true);
    r.signed("opened", 65, OLD);
    let old_box = r.one_at(id, 65, OLD).state.box_id().unwrap();
    assert!(r.store_gates(old_box), "the Box holds the gates");
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    let old = r.one_at(id, 65, OLD);
    let sha = CommitSha::parse(NEW).unwrap();
    assert_eq!(cancelled(&old), Some(&CancelCause::NewerHead { sha }));
    let new = r.one_at(id, 65, NEW);
    assert!(
        matches!(new.state, RunState::Running { .. }),
        "{:?}",
        new.state
    );
    r.pause(MIN);
    r.pass();
    assert_eq!(
        r.one_at(id, 65, OLD).state,
        old.state,
        "a cancelled run stays so"
    );
    assert!(matches!(
        r.one_at(id, 65, NEW).state,
        RunState::Running { .. }
    ));
}

#[test]
fn the_gate_pass_leaves_the_box_of_a_cancelled_run_alone() {
    let r = hooked();
    let id = r.put_gated_for(&[proof()], true);
    r.signed("opened", 65, OLD);
    let old_box = r.one_at(id, 65, OLD).state.box_id().unwrap();
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    assert!(cancelled(&r.one_at(id, 65, OLD)).is_some());
    r.pause(10 * MIN);
    let store = r.cx.store().unwrap();
    let out = crate::ops::gate_pass(&r.cx, store, &|| false, &|_| {}).unwrap();
    assert!(out.is_empty(), "{out:?}");
    assert!(
        store.unsent(old_box).unwrap().is_empty(),
        "no reminder for a run that was cancelled"
    );
    assert!(store.gate_lines(old_box).unwrap().is_empty());
}

impl Rig {
    /// Whether Box `id` holds gates.
    fn store_gates(&self, id: moor_core::BoxId) -> bool {
        self.cx.store().unwrap().box_gates(id).unwrap().is_some()
    }

    /// `answer_run` for the workflow, as its owner.
    fn answer_the_run(
        &self,
        id: WorkflowId,
        answer: Answer,
    ) -> Result<moor_core::view::workflow::WorkflowAnswerValue, crate::error::CommandError> {
        let store = self.cx.store().unwrap();
        let name = store.workflow_by_id(id).unwrap().unwrap().name;
        crate::ops::workflow::answer_run(
            &self.cx,
            store,
            REQUESTER_USER,
            (name.as_str(), None),
            (answer, None),
        )
    }
}
