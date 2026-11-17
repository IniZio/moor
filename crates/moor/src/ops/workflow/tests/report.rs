//! The agent's report of a step: every reply of the table in the spec, the code gate, and the
//! reminder that a quiet agent with no report gets.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use moor_core::request::StepReportReq;
use moor_core::view::workflow::StepReportValue;
use moor_core::workflow::{
    Plan, RunEnd, RunOutcome, RunState, StepId, StepKind, StepReport, StepResult, WorkflowId,
};
use moor_core::BoxId;
use serde_json::json;

use super::plan::{step_of, PROMPT};
use super::{rig, Rig, FIRST_DUE};
use crate::ops::workflow::step_report;

/// An index line, as the Box's index script prints it, for a file of `size` bytes.
fn listing(name: &str, size: u64) -> String {
    format!("{name}\t{size}\t{}\n", "a".repeat(64))
}

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

fn plan_of(steps: &[serde_json::Value]) -> Plan {
    Plan::parse(&json!({ "steps": steps }).to_string(), PROMPT).unwrap()
}

fn audit(ok: &str, fail: Option<&str>) -> serde_json::Value {
    json!({"id": "audit", "title": "Audit", "kind": "work", "instructions": "look",
           "done_when": "looked", "evidence": ["a.md", "shots-*"], "ok": ok, "fail": fail})
}

fn ship() -> serde_json::Value {
    json!({"id": "ship", "title": "Ship", "kind": "deliver",
           "instructions": "ship it", "done_when": "shipped"})
}

fn gate() -> serde_json::Value {
    json!({"id": "gate", "title": "Gate", "kind": "check", "command": "true", "ok": "ship"})
}

fn approve() -> serde_json::Value {
    json!({"id": "ask", "title": "Ask", "kind": "approve", "question": "go?", "ok": "ship"})
}

fn ok(step: &str, note: &str) -> StepReport {
    StepReport {
        step: id(step),
        result: StepResult::Ok,
        evidence: vec!["a.md".to_owned()],
        note: note.to_owned(),
    }
}

fn fail(step: &str, note: &str) -> StepReport {
    StepReport {
        result: StepResult::Fail,
        ..ok(step, note)
    }
}

/// The files that satisfy `audit`.
fn files() -> String {
    format!("{}{}", listing("a.md", 5), listing("shots-1.png", 9))
}

impl Rig {
    /// Starts the run of a planned workflow, its agent working on the first step.
    pub(super) fn begun(&self, plan: Plan) -> WorkflowId {
        self.answer_repo();
        let wid = self.put_planned(Some(plan));
        self.advance_to(FIRST_DUE + 1_000);
        assert!(self.pass().is_empty());
        wid
    }

    /// What the agent in the newest run's Box is told for `report`.
    pub(super) fn report(&self, wid: WorkflowId, report: StepReport) -> StepReportValue {
        self.report_from(self.latest_box(wid), report)
    }

    /// What the agent in Box `box_id` is told for `report`.
    pub(super) fn report_from(&self, box_id: BoxId, report: StepReport) -> StepReportValue {
        step_report(&self.cx, &StepReportReq { box_id, report }).unwrap()
    }

    /// The agent reports the step the run is at as done, leaving what the step asks for.
    pub(super) fn report_current(&self, wid: WorkflowId) -> StepReportValue {
        let run = self.latest(wid);
        let (at, _) = step_of(&run.state);
        let plan = self.cx.store().unwrap().run_plan(run.id, PROMPT).unwrap();
        let (listed, note) = match plan.step(&id(&at)).map(|s| &s.kind) {
            Some(StepKind::Work { evidence, .. }) => (
                evidence
                    .names()
                    .map(|n| listing(&n.as_str().replace('*', "1"), 5))
                    .collect::<String>(),
                "",
            ),
            _ => (String::new(), "all done"),
        };
        self.driver.push_artifact_index(listed);
        self.report(wid, ok(&at, note))
    }

    /// The history lines of the newest run, as their kinds.
    fn trail(&self, wid: WorkflowId) -> Vec<String> {
        self.kinds(wid)
    }
}

#[test]
fn a_box_with_no_running_run_is_told_so_and_nothing_changes() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    let nobody = r.report_from(BoxId::new(4_242), ok("audit", ""));
    assert!(nobody.error, "{nobody:?}");
    assert!(
        nobody.reply.contains("no running workflow run"),
        "{nobody:?}"
    );
    assert_eq!(r.trail(wid), ["queued", "entered"]);
}

#[test]
fn a_step_that_is_not_the_current_one_gets_the_current_step_and_nothing_changes() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    let told = r.report(wid, ok("ship", "done"));
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("current step is audit"), "{told:?}");
    assert!(told.reply.contains("Evidence: a.md, shots-*"), "{told:?}");
    assert_eq!(r.trail(wid), ["queued", "entered"]);
}

#[test]
fn a_refused_report_says_what_is_missing_and_a_second_one_fails_the_step() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    r.driver.push_artifact_index(listing("a.md", 0));
    let told = r.report(wid, ok("audit", ""));
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("`a.md` is empty"), "{told:?}");
    assert!(told.reply.contains("no file matches `shots-*`"), "{told:?}");
    assert!(told.reply.contains("A second refusal"), "{told:?}");
    assert_eq!(r.trail(wid), ["queued", "entered", "gate_refused"]);
    assert_eq!(step_of(&r.latest(wid).state), ("audit".to_owned(), 1));

    r.driver
        .push_artifact_index(format!("{}{}", listing("a.md", 26 * 1024 * 1024), "x"));
    let told = r.report(wid, ok("audit", ""));
    assert!(!told.error, "{told:?}");
    assert!(told.reply.contains("refused twice"), "{told:?}");
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Failed { at, reason },
        ..
    }) = r.latest(wid).state
    else {
        panic!("{:?}", r.latest(wid).state);
    };
    assert_eq!(at.as_str(), "audit");
    assert!(reason.contains("refused twice"), "{reason}");
    assert_eq!(
        r.trail(wid),
        ["queued", "entered", "gate_refused", "gate_refused", "ended"]
    );
}

#[test]
fn a_file_over_the_limit_is_refused_by_size() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    let big = format!(
        "{}{}",
        listing("a.md", 25 * 1024 * 1024 + 1),
        listing("shots-1.png", 1)
    );
    r.driver.push_artifact_index(big);
    let told = r.report(wid, ok("audit", ""));
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("25 MiB"), "{told:?}");
    let exact = format!(
        "{}{}",
        listing("a.md", 25 * 1024 * 1024),
        listing("shots-1.png", 1)
    );
    r.driver.push_artifact_index(exact);
    assert!(!r.report(wid, ok("audit", "")).error);
}

#[test]
fn a_passed_report_followed_by_work_or_deliver_sends_that_steps_instructions() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    r.driver.push_artifact_index(files());
    let told = r.report(wid, ok("audit", ""));
    assert!(!told.error, "{told:?}");
    assert!(
        told.reply.contains("Current step: ship, \"Ship\""),
        "{told:?}"
    );
    assert!(told.reply.contains("ship it"), "{told:?}");
    assert!(told.reply.contains("report_step"), "{told:?}");
    assert_eq!(step_of(&r.latest(wid).state), ("ship".to_owned(), 1));
    assert_eq!(
        r.trail(wid),
        ["queued", "entered", "reported", "entered", "sent"]
    );
}

#[test]
fn a_passed_report_followed_by_a_check_says_to_end_the_turn() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("gate", None), gate(), ship()]));
    r.driver.push_artifact_index(files());
    let told = r.report(wid, ok("audit", ""));
    assert!(!told.error, "{told:?}");
    assert_eq!(
        told.reply,
        "End your turn. moor runs the check and sends you the result."
    );
    assert_eq!(step_of(&r.latest(wid).state), ("gate".to_owned(), 1));
    assert_eq!(r.trail(wid), ["queued", "entered", "reported", "entered"]);
}

#[test]
fn a_passed_report_followed_by_an_approve_says_moor_asks_the_owner() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ask", None), approve(), ship()]));
    r.driver.push_artifact_index(files());
    let told = r.report(wid, ok("audit", ""));
    assert_eq!(
        told.reply,
        "End your turn. moor asks the owner and sends you the answer."
    );
    assert_eq!(step_of(&r.latest(wid).state), ("ask".to_owned(), 1));
}

#[test]
fn a_deliver_report_ends_the_run_with_its_note_as_the_result() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    r.driver.push_artifact_index(files());
    assert!(!r.report(wid, ok("audit", "")).error);

    let told = r.report(wid, ok("ship", "  "));
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("needs a note"), "{told:?}");

    let told = r.report(wid, ok("ship", "opened PR 12, same as last week"));
    assert_eq!(told.reply, "Done.");
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Delivered { result },
        ..
    }) = r.latest(wid).state
    else {
        panic!("{:?}", r.latest(wid).state);
    };
    assert_eq!(result, "opened PR 12, same as last week");
    let kinds = r.trail(wid);
    assert_eq!(kinds.last().map(String::as_str), Some("ended"), "{kinds:?}");
}

#[test]
fn the_same_report_again_is_already_recorded_and_the_next_step_comes_again() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    r.driver.push_artifact_index(files());
    let first = r.report(wid, ok("audit", ""));
    let before = r.trail(wid);
    let again = r.report(wid, ok("audit", ""));
    assert!(!again.error, "{again:?}");
    assert!(again.reply.starts_with("Already recorded."), "{again:?}");
    assert!(again.reply.contains("Current step: ship"), "{again:?}");
    assert!(first.reply.contains("Current step: ship"), "{first:?}");
    assert_eq!(r.trail(wid), before);
}

#[test]
fn a_fail_report_needs_a_note_and_follows_the_fail_edge() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", Some("ship")), ship()]));
    let told = r.report(wid, fail("audit", ""));
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("needs a note"), "{told:?}");

    let told = r.report(wid, fail("audit", "the registry is down"));
    assert!(!told.error, "{told:?}");
    assert!(told.reply.contains("Current step: ship"), "{told:?}");
    assert_eq!(step_of(&r.latest(wid).state), ("ship".to_owned(), 1));
}

#[test]
fn a_fail_report_with_no_fail_edge_fails_the_run_with_the_note() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    let told = r.report(wid, fail("audit", "the registry is down"));
    assert!(told.reply.contains("the registry is down"), "{told:?}");
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Failed { at, reason },
        ..
    }) = r.latest(wid).state
    else {
        panic!("{:?}", r.latest(wid).state);
    };
    assert_eq!(at.as_str(), "audit");
    assert!(reason.contains("the registry is down"), "{reason}");
}

#[test]
fn a_step_moor_runs_is_not_the_agents_to_report() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("gate", None), gate(), ship()]));
    r.driver.push_artifact_index(files());
    assert!(!r.report(wid, ok("audit", "")).error);
    let told = r.report(wid, ok("gate", "done"));
    assert!(told.error, "{told:?}");
    assert!(told.reply.contains("End your turn"), "{told:?}");
    assert_eq!(step_of(&r.latest(wid).state), ("gate".to_owned(), 1));
}

#[test]
fn a_quiet_agent_with_no_report_is_reminded_once_across_a_restart_and_then_incomplete() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    r.quiet_after_work(wid);
    r.pause(3 * 60_000);
    assert!(r.pass().is_empty());
    assert_eq!(step_of(&r.latest(wid).state), ("audit".to_owned(), 1));
    assert_eq!(r.trail(wid), ["queued", "entered", "reminded"]);

    // A controller that restarts reads the same line and does not remind again.
    let again = r.restarted();
    again.quiet_after_work(wid);
    again.pause(3 * 60_000);
    assert!(again.pass().is_empty());
    let RunState::Done(RunEnd::Ended {
        outcome: RunOutcome::Incomplete { at },
        ..
    }) = again.latest(wid).state
    else {
        panic!("{:?}", again.latest(wid).state);
    };
    assert_eq!(at.as_str(), "audit");
    assert_eq!(again.trail(wid), ["queued", "entered", "reminded", "ended"]);
}

#[test]
fn a_report_after_the_reminder_still_moves_the_run_and_the_counter_starts_over() {
    let r = rig();
    let wid = r.begun(plan_of(&[audit("ship", None), ship()]));
    r.quiet_after_work(wid);
    r.pause(3 * 60_000);
    r.pass();
    assert_eq!(r.trail(wid), ["queued", "entered", "reminded"]);
    r.driver.push_artifact_index(files());
    assert!(!r.report(wid, ok("audit", "")).error);
    // The deliver step is quiet now with no report: it gets its own reminder first.
    r.quiet_after_work(wid);
    r.pause(3 * 60_000);
    r.pass();
    assert_eq!(step_of(&r.latest(wid).state), ("ship".to_owned(), 1));
    assert_eq!(r.trail(wid).last().map(String::as_str), Some("reminded"));
}

#[test]
fn a_workflow_nobody_planned_still_delivers_when_its_agent_goes_quiet() {
    let r = rig();
    r.answer_repo();
    let wid = r.put_planned(None);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    r.quiet_after_work(wid);
    r.pause(3 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(wid).state,
        RunState::Done(RunEnd::Ended {
            outcome: RunOutcome::Delivered { .. },
            ..
        })
    ));
}
