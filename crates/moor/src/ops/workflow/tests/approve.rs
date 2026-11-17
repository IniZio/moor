//! Approve steps: a run waits for its owner, the answer moves it, and a wait with no answer ends
//! declined after a week.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use moor_core::user::UserId;
use moor_core::view::workflow::WorkflowAnswerValue;
use moor_core::workflow::{Answer, Plan, RunEnd, RunOutcome, RunState, WorkflowId, WorkflowRun};
use serde_json::json;

use super::plan::{step_of, PROMPT};
use super::previous::put_to_sleep;
use super::{rig, Rig, FIRST_DUE};
use crate::error::CommandError;
use crate::ops::workflow::answer_run;
use crate::testing::cx::REQUESTER_USER;

const MIN: i64 = 60_000;
const DAY: i64 = 24 * 60 * MIN;

fn plan_of(steps: &[serde_json::Value]) -> Plan {
    Plan::parse(&json!({ "steps": steps }).to_string(), PROMPT).unwrap()
}

fn audit() -> serde_json::Value {
    json!({"id": "audit", "title": "Audit", "kind": "work", "instructions": "look",
           "done_when": "looked", "evidence": ["a.md"], "ok": "ask"})
}

fn ask(fail: Option<&str>) -> serde_json::Value {
    json!({"id": "ask", "title": "Ask", "kind": "approve", "question": "ship it?",
           "ok": "ship", "fail": fail})
}

fn redo() -> serde_json::Value {
    json!({"id": "redo", "title": "Redo", "kind": "work", "instructions": "again",
           "done_when": "done", "evidence": ["b.md"], "ok": "ship"})
}

fn ship() -> serde_json::Value {
    json!({"id": "ship", "title": "Ship", "kind": "deliver",
           "instructions": "ship it", "done_when": "shipped"})
}

impl Rig {
    /// A run that did its first step and waits at `ask`, its Box asleep.
    fn waiting(&self, steps: &[serde_json::Value]) -> WorkflowId {
        let wid = self.begun(plan_of(steps));
        assert!(!self.report_current(wid).error);
        self.quiet_after_work(wid);
        put_to_sleep(self, self.latest_box(wid));
        wid
    }

    /// The first run of the workflow; later ones are skips or fresh firings.
    fn first(&self, wid: WorkflowId) -> WorkflowRun {
        self.runs(wid).into_iter().next_back().unwrap()
    }

    fn lines_of(&self, run: &WorkflowRun) -> Vec<String> {
        self.cx
            .store()
            .unwrap()
            .run_history(run.id)
            .unwrap()
            .iter()
            .map(|l| l.entry["kind"].as_str().unwrap().to_owned())
            .collect()
    }

    fn answer_as(
        &self,
        user: UserId,
        wid: WorkflowId,
        answer: Answer,
        note: Option<&str>,
    ) -> Result<WorkflowAnswerValue, CommandError> {
        let store = self.cx.store().unwrap();
        let name = store.workflow_by_id(wid).unwrap().unwrap().name;
        answer_run(&self.cx, store, user, (name.as_str(), "o/n", answer, note))
    }

    fn answer(&self, wid: WorkflowId, answer: Answer) -> Result<WorkflowAnswerValue, CommandError> {
        self.answer_as(REQUESTER_USER, wid, answer, None)
    }

    /// What is queued for the Box of the first run, as text.
    fn queued(&self, wid: WorkflowId) -> String {
        let id = self.first(wid).state.box_id().unwrap();
        let rows = self.cx.store().unwrap().unsent(id).unwrap();
        rows.into_iter()
            .map(|r| r.text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn ended(run: &WorkflowRun) -> &RunOutcome {
    match &run.state {
        RunState::Done(RunEnd::Ended { outcome, .. }) => outcome,
        other => panic!("not ended: {other:?}"),
    }
}

#[test]
fn entering_an_approve_step_from_a_report_sets_waiting_and_the_box_is_left_to_sleep() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let run = r.first(wid);
    let RunState::Waiting { ref at, try_n, .. } = run.state else {
        panic!("{:?}", run.state);
    };
    assert_eq!((at.as_str(), try_n), ("ask", 1));
    assert_eq!(
        r.lines_of(&run),
        ["queued", "entered", "reported", "entered"]
    );
    // The tick leaves it be, however long the Box sleeps.
    r.advance_to(FIRST_DUE + 2 * DAY);
    r.pass();
    assert!(matches!(r.first(wid).state, RunState::Waiting { .. }));
}

#[test]
fn a_run_that_begins_at_an_approve_step_waits_from_the_tick() {
    let r = rig();
    r.answer_repo();
    let wid = r.put_planned(Some(plan_of(&[ask(None), ship()])));
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    r.pause(MIN);
    assert!(r.pass().is_empty());
    let run = r.first(wid);
    assert!(
        matches!(run.state, RunState::Waiting { .. }),
        "{:?}",
        run.state
    );
    assert_eq!(r.lines_of(&run), ["queued", "entered"]);
}

#[test]
fn yes_goes_on_with_the_next_step_sent_to_the_sleeping_box_and_the_run_delivers() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let told = r
        .answer_as(REQUESTER_USER, wid, Answer::Yes, Some("  go, but squash  "))
        .unwrap();
    assert_eq!(told.repo, "o/n");
    let run = r.first(wid);
    assert_eq!(step_of(&run.state), ("ship".to_owned(), 1));
    assert!(matches!(run.state, RunState::Running { .. }));
    assert_eq!(
        r.lines_of(&run),
        ["queued", "entered", "reported", "entered", "answered", "entered", "sent"]
    );
    let steer = r.queued(wid);
    assert!(steer.contains("The owner answered yes"), "{steer}");
    assert!(steer.contains("go, but squash"), "{steer}");
    assert!(steer.contains("Current step: ship"), "{steer}");

    let told = r.report(wid, super::report::ok("ship", "opened PR 3"));
    assert_eq!(told.reply, "Done.");
    assert!(matches!(
        ended(&r.first(wid)),
        RunOutcome::Delivered { result } if result == "opened PR 3"
    ));
}

#[test]
fn no_with_a_fail_edge_goes_along_it() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(Some("redo")), redo(), ship()]);
    r.answer(wid, Answer::No).unwrap();
    let run = r.first(wid);
    assert_eq!(step_of(&run.state), ("redo".to_owned(), 1));
    assert!(r.queued(wid).contains("The owner answered no"));
}

#[test]
fn no_without_a_fail_edge_ends_the_run_declined_and_the_next_firing_is_not_held_off() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    r.answer(wid, Answer::No).unwrap();
    let run = r.first(wid);
    assert_eq!(
        *ended(&run),
        RunOutcome::Declined {
            at: "ask".parse_step()
        }
    );
    assert_eq!(
        r.lines_of(&run),
        ["queued", "entered", "reported", "entered", "answered", "ended"]
    );
    r.advance_to(FIRST_DUE + DAY + 1_000);
    r.pass();
    let next = r.latest(wid);
    assert!(
        matches!(next.state, RunState::Running { .. }),
        "{:?}",
        next.state
    );
}

trait ParseStep {
    fn parse_step(&self) -> moor_core::workflow::StepId;
}

impl ParseStep for str {
    fn parse_step(&self) -> moor_core::workflow::StepId {
        moor_core::workflow::StepId::parse(self).unwrap()
    }
}

#[test]
fn only_the_owner_may_answer_and_a_run_not_waiting_cannot_be_answered() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let why = r
        .answer_as(UserId::new(99), wid, Answer::Yes, None)
        .unwrap_err()
        .error
        .to_string();
    assert!(why.contains("no workflow named"), "{why}");
    assert!(matches!(r.first(wid).state, RunState::Waiting { .. }));
    assert_eq!(r.lines_of(&r.first(wid)).len(), 4);

    r.answer(wid, Answer::Yes).unwrap();
    let again = r.answer(wid, Answer::Yes).unwrap_err().error.to_string();
    assert!(again.contains("waiting for an answer"), "{again}");
}

#[test]
fn a_wait_ends_declined_after_seven_days_with_no_answer() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let since = r.cx.clock().now().as_millis();
    r.advance_to(since + 7 * DAY - MIN);
    r.pass();
    assert!(matches!(r.first(wid).state, RunState::Waiting { .. }));
    r.advance_to(since + 7 * DAY + MIN);
    r.pass();
    let run = r.first(wid);
    assert!(matches!(ended(&run), RunOutcome::Declined { .. }));
    let kinds = r.lines_of(&run);
    assert_eq!(&kinds[kinds.len() - 2..], ["unanswered", "ended"]);
}

#[test]
fn a_waiting_run_holds_off_the_next_firing_and_does_not_time_out() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    r.advance_to(FIRST_DUE + 2 * DAY + 1_000);
    r.pass();
    assert!(matches!(r.first(wid).state, RunState::Waiting { .. }));
    assert_eq!(r.latest(wid).state, RunState::Done(RunEnd::SkippedOverlap));
}

#[test]
fn the_timeout_does_not_run_while_waiting() {
    let r = rig();
    r.answer_repo();
    let wid = r.begun(plan_of(&[audit(), ask(None), ship()]));
    // 170 of the 180 minutes are worked before the approve step.
    r.pause(170 * MIN);
    assert!(!r.report_current(wid).error);
    r.quiet_after_work(wid);
    r.pause(3 * DAY);
    r.pass();
    r.answer(wid, Answer::Yes).unwrap();
    r.pause(5 * MIN);
    r.pass();
    assert!(
        matches!(r.first(wid).state, RunState::Running { .. }),
        "{:?}",
        r.first(wid).state
    );
    r.pause(6 * MIN);
    r.pass();
    assert!(matches!(ended(&r.first(wid)), RunOutcome::TimedOut { .. }));
}

#[test]
fn an_answer_racing_the_weeks_expiry_applies_once_and_the_expiry_wins_when_first() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let since = r.cx.clock().now().as_millis();
    r.advance_to(since + 7 * DAY + MIN);
    // The tick expires the wait between the answer's read and its move.
    super::super::answer::tests_hook::once_after_read(|cx| {
        let run = cx
            .store()
            .unwrap()
            .open_workflow_runs(WorkflowId::new(1))
            .unwrap();
        let w = cx
            .store()
            .unwrap()
            .workflow_by_id(WorkflowId::new(1))
            .unwrap()
            .unwrap();
        let now = cx.clock().now();
        super::super::settle::expire_runs(cx, cx.store().unwrap(), &w, now, &mut Vec::new())
            .unwrap();
        assert_eq!(run.len(), 1);
    });
    let why = r.answer(wid, Answer::Yes).unwrap_err().error.to_string();
    assert!(why.contains("moved while you answered"), "{why}");
    let run = r.first(wid);
    assert!(matches!(ended(&run), RunOutcome::Declined { .. }));
    assert!(!r.lines_of(&run).contains(&"answered".to_owned()));
}

#[test]
fn an_answer_that_won_the_race_is_not_undone_by_the_expiry() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let since = r.cx.clock().now().as_millis();
    r.answer(wid, Answer::Yes).unwrap();
    r.advance_to(since + 7 * DAY + MIN);
    // The run now works at ship, so a week-old wait finds nothing to end.
    r.pass();
    assert!(!matches!(
        r.first(wid).state,
        RunState::Done(RunEnd::Ended {
            outcome: RunOutcome::Declined { .. },
            ..
        })
    ));
}

#[test]
fn a_closed_box_ends_a_waiting_run_incomplete() {
    let r = rig();
    let wid = r.waiting(&[audit(), ask(None), ship()]);
    let id = r.first(wid).state.box_id().unwrap();
    let store = r.cx.store().unwrap();
    let name = store.box_row(id).unwrap().unwrap().name;
    crate::ops::close::run(
        &r.cx,
        &moor_core::CloseReq {
            target: name.into(),
            force: true,
        },
    )
    .unwrap();
    r.pass();
    assert!(matches!(
        ended(&r.first(wid)),
        RunOutcome::Incomplete { .. }
    ));
}
