//! The history of a gated run: what its gate lines add, in the order they came.
#![allow(
    clippy::indexing_slicing,
    reason = "a test indexes the JSON it just read"
)]

use std::cell::RefCell;

use moor_core::gate::{GateDoc, GateLine};
use moor_core::workflow::{StepReport, StepResult, WorkflowId};

use super::super::FIRST_DUE;
use super::{proof, reported, rig, step, Rig};
use crate::ops::workflow::{resolve_interrupted, tick};

/// The gate `done`: the agent leaves `posted.md`, and a check passes.
fn done() -> GateDoc {
    GateDoc {
        name: "done".to_owned(),
        evidence: vec!["posted.md".to_owned()],
        checks: vec!["true".into()],
        approval: None,
        max_tries: None,
    }
}

impl Rig {
    /// The newest run's history as `kind step try`, oldest first.
    pub(super) fn told(&self, id: WorkflowId) -> Vec<String> {
        let run = self.latest(id);
        let lines = self.cx.store().unwrap().run_history(run.id).unwrap();
        lines
            .iter()
            .map(|l| {
                let part = |k: &str| {
                    l.entry.get(k).map(|v| {
                        format!(
                            " {}",
                            v.as_str().map_or_else(|| v.to_string(), str::to_owned)
                        )
                    })
                };
                let kind = l.entry["kind"].as_str().unwrap();
                format!(
                    "{kind}{}{}",
                    part("step").unwrap_or_default(),
                    part("try_n").unwrap_or_default()
                )
            })
            .collect()
    }
}

#[test]
fn a_gated_run_starts_at_its_first_gate_and_never_names_deliver() {
    let r = rig();
    let id = r.started_gated(&[proof(), done()]);
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
    assert_eq!(r.told(id), ["queued", "entered proof 1"]);
}

#[test]
fn the_history_of_a_gated_run_reads_in_the_order_the_lines_came() {
    let r = rig();
    let id = r.started_gated(&[proof(), done()]);
    for line in [
        reported("proof", "proof in"),
        reported("done", ""),
        GateLine::Checked {
            gate: step("done"),
            try_n: 1,
            check: 0,
            exit: 0,
            tail: String::new(),
        },
    ] {
        r.gate_line(id, &line);
    }
    r.pass();
    assert_eq!(
        r.told(id),
        [
            "queued",
            "entered proof 1",
            "reported proof 1",
            "gate_passed proof",
            "entered done 1",
            "reported done 1",
            "checked done 1",
            "gate_passed done",
            "outcome_reached",
            "ended",
        ]
    );
}

#[test]
fn a_stuck_gate_keeps_its_try_when_the_run_waits_and_the_history_says_so() {
    let r = rig();
    let two = GateDoc {
        max_tries: Some(2),
        ..proof()
    };
    let id = r.started_gated(&[two]);
    r.gate_line(
        id,
        &GateLine::Submitted {
            gate: step("proof"),
            try_n: 1,
            report: StepReport {
                step: step("proof"),
                result: StepResult::Fail,
                evidence: Vec::new(),
                note: "cannot".to_owned(),
            },
        },
    );
    r.pass();
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 2)));
    gone_quiet_at(id, &r, "proof", 2);
    r.pass();
    assert_eq!(r.at(id), ("waiting", ("proof".to_owned(), 2)));
    r.gate_line(
        id,
        &GateLine::Reopened {
            gate: step("proof"),
            by: "me".to_owned(),
        },
    );
    r.pass();
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
    assert_eq!(
        r.told(id),
        [
            "queued",
            "entered proof 1",
            "reported proof 1",
            "entered proof 2",
            "reminded proof 2",
            "gate_waiting proof 2",
            "resumed proof",
            "entered proof 1",
        ]
    );
}

#[test]
fn a_gate_stuck_before_the_tick_caught_up_waits_at_the_try_it_ended_in() {
    let r = rig();
    let two = GateDoc {
        max_tries: Some(2),
        ..proof()
    };
    let id = r.started_gated(&[two]);
    let failed = |try_n| GateLine::Submitted {
        gate: step("proof"),
        try_n,
        report: StepReport {
            step: step("proof"),
            result: StepResult::Fail,
            evidence: Vec::new(),
            note: "cannot".to_owned(),
        },
    };
    r.gate_line(id, &failed(1));
    r.gate_line(id, &failed(2));
    r.pass();
    assert_eq!(r.at(id), ("waiting", ("proof".to_owned(), 2)));
    assert_eq!(
        r.told(id),
        [
            "queued",
            "entered proof 1",
            "reported proof 1",
            "entered proof 2",
            "reported proof 2",
            "gate_waiting proof 2",
        ]
    );
}

/// The agent was reminded and went quiet again at try `try_n` of `gate`.
fn gone_quiet_at(id: WorkflowId, r: &Rig, gate: &str, try_n: u8) {
    r.gate_line(
        id,
        &GateLine::Reminded {
            gate: step(gate),
            try_n,
        },
    );
    r.gate_line(
        id,
        &GateLine::Quiet {
            gate: step(gate),
            try_n,
        },
    );
}

#[test]
fn a_recovered_run_of_a_gated_workflow_starts_at_its_first_gate() {
    let r = rig();
    r.answer_repo();
    let id = r.put_gated(&[proof(), done()]);
    r.advance_to(FIRST_DUE + 1_000);
    let job = RefCell::new(None);
    tick(&r.cx, (&|j| *job.borrow_mut() = Some(j), &|_| {}), &|| {
        false
    });
    let job = job.into_inner().unwrap();
    let req = moor_core::StartReq {
        place: false,
        gates: job.firing.gates.clone().map(Box::new),
        placement: moor_core::decide::schedule::Placement::FirstOnly,
        cold: false,
        config: None,
        name: job.firing.name.clone(),
        prompt: Some(job.firing.prompt.clone()),
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        resume: false,
        repo: Some(job.firing.repo),
        also: None,
        parent: None,
        identity: None,
    };
    crate::ops::start::run(&r.cx, &req).unwrap();
    resolve_interrupted(&r.cx);
    assert_eq!(r.at(id), ("running", ("proof".to_owned(), 1)));
    assert_eq!(r.told(id), ["queued", "entered proof 1"]);
}

#[test]
fn a_run_waiting_because_moor_got_no_answer_keeps_the_cause_as_data() {
    let r = rig();
    let id = r.started_gated(&[done()]);
    let lost = GateLine::NoAnswer {
        gate: step("done"),
        try_n: 1,
        check: moor_core::gate::CheckOf::Gate { check: 0 },
        why: moor_core::gate::Unanswered::Lost,
    };
    for line in [reported("done", ""), lost.clone(), lost] {
        r.gate_line(id, &line);
    }
    r.pass();
    let run = r.latest(id);
    let lines = r.cx.store().unwrap().run_history(run.id).unwrap();
    let waiting = lines
        .iter()
        .find(|l| l.entry["kind"] == "gate_waiting")
        .expect("the run waits");
    assert_eq!(
        waiting.entry["unanswered"],
        serde_json::json!({"check": {"gate": {"check": 0}}, "command": "true", "reason": {"kind": "lost"}})
    );
}
