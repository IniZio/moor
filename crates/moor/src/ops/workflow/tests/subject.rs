//! One open run per workflow and subject: pull requests run beside each other, a newer head
//! cancels its pull request's older run and stops its agent, and a run closes only its own
//! subject's earlier Boxes.
#![allow(
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "a test names what it found when it fails, and walks one scenario step by step"
)]

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::view::workflow::RunNowValue;
use moor_core::workflow::{
    Answer, CancelCause, CommitSha, Plan, PullTarget, RunEnd, RunOutcome, RunState, WorkflowId,
    WorkflowRun,
};
use moor_core::BoxId;
use serde_json::json;

use super::plan::PROMPT;
use super::previous::{lifecycle, put_to_sleep, rig_probing};
use super::report::ok;
use super::{repo, rig, Rig, FIRST_DUE};
use crate::ops::workflow::cancel::cancel_pull;
use crate::ops::workflow::now::run_with;
use crate::ops::workflow::{answer_run, run_start};
use crate::testing::cx::REQUESTER_USER;

/// One head of a pull request.
const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// A head pushed after [`OLD`].
const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn target(number: u64) -> PullTarget {
    PullTarget::parse(&format!("o/n#{number}")).unwrap()
}

impl Rig {
    /// `run_workflow` of `workflow` for pull request `number` of the repo, its head read as
    /// `head` and its Box started on the spot.
    fn fire_pull(&self, workflow: &str, number: u64, head: &str) -> RunNowValue {
        let read = |_: &PullTarget| Ok(CommitSha::parse(head).unwrap());
        let start = |job| {
            run_start(&self.cx, &job);
            Ok(())
        };
        let pull = target(number);
        let store = self.cx.store().unwrap();
        let args = (workflow, None, Some(&pull));
        run_with(&self.cx, store, REQUESTER_USER, args, (&read, &start)).unwrap()
    }

    /// The newest run of `id` for pull request `number` at `head`.
    fn run_for(&self, id: WorkflowId, number: u64, head: &str) -> WorkflowRun {
        self.runs(id)
            .into_iter()
            .find(|r| {
                r.cause
                    .subject()
                    .is_some_and(|p| p.number == number && p.head_sha.as_str() == head)
            })
            .unwrap_or_else(|| panic!("no run for #{number} at {head}"))
    }

    fn session(&self, id: BoxId) -> SessionState {
        self.cx
            .store()
            .unwrap()
            .box_row(id)
            .unwrap()
            .unwrap()
            .session
    }
}

fn started(line: &RunNowValue) {
    assert_eq!(line.status, "started", "{line:?}");
}

/// The step the single plan's run is at.
fn single() -> moor_core::workflow::StepId {
    Plan::single_id()
}

#[test]
fn two_pull_requests_run_at_once_and_beside_the_workflows_own_run() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(5_000);
    started(&r.fire_pull("daily-fix", 65, OLD));
    started(&r.fire_pull("daily-fix", 66, OLD));
    let open = r.cx.store().unwrap().open_workflow_runs(id).unwrap();
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(open
        .iter()
        .all(|r| matches!(r.state, RunState::Running { .. })));
    assert_eq!(r.open_boxes(), 2);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let cron = r.latest(id);
    assert!(cron.cause.subject().is_none());
    assert!(
        matches!(cron.state, RunState::Running { .. }),
        "the schedule is not held off by the pull requests' runs: {:?}",
        cron.state
    );
    assert_eq!(r.open_boxes(), 3);
}

#[test]
fn the_same_head_again_is_refused_while_its_run_is_open() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    started(&r.fire_pull("daily-fix", 65, OLD));
    let again = r.fire_pull("daily-fix", 65, OLD);
    assert_eq!(again.status, "refused", "{again:?}");
    let why = again.reason.unwrap_or_default();
    assert!(
        why.contains("of daily-fix for o/n#65 is still open"),
        "{why}"
    );
    assert_eq!(r.runs(id).len(), 1);
}

#[test]
fn a_newer_head_cancels_the_older_run_and_stops_its_agent_and_leaves_other_pulls_be() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(5_000);
    // All three in one millisecond, so the runs share a due time and only their ids tell them apart.
    started(&r.fire_pull("daily-fix", 65, OLD));
    started(&r.fire_pull("daily-fix", 66, OLD));
    let old_box = r.run_for(id, 65, OLD).state.box_id().unwrap();
    assert_ne!(r.session(old_box), SessionState::Ended);
    started(&r.fire_pull("daily-fix", 65, NEW));

    let old = r.run_for(id, 65, OLD);
    let RunState::Done(RunEnd::Ended {
        box_id,
        outcome: RunOutcome::Cancelled { at, cause },
        ..
    }) = &old.state
    else {
        panic!("not cancelled: {:?}", old.state);
    };
    assert_eq!((*box_id, at), (old_box, &single()));
    let sha = CommitSha::parse(NEW).unwrap();
    assert_eq!(cause, &CancelCause::NewerHead { sha });
    assert_eq!(
        r.session(old_box),
        SessionState::Ended,
        "its agent is stopped"
    );
    assert_ne!(
        lifecycle(&r, old_box),
        BoxLifecycle::Closed,
        "its Box stays"
    );
    assert!(matches!(
        r.run_for(id, 65, NEW).state,
        RunState::Running { .. }
    ));
    assert!(matches!(
        r.run_for(id, 66, OLD).state,
        RunState::Running { .. }
    ));

    let late = r.report_from(old_box, ok(single().as_str(), "done"));
    assert!(late.error, "{late:?}");
    assert_eq!(r.run_for(id, 65, OLD).state, old.state);
}

#[test]
fn closing_a_pull_request_cancels_its_open_run_once() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    started(&r.fire_pull("daily-fix", 65, OLD));
    started(&r.fire_pull("daily-fix", 66, OLD));
    let store = r.cx.store().unwrap();
    let w = store.workflow_by_id(id).unwrap().unwrap();
    let close = || cancel_pull(&r.cx, store, &w, (&repo(), 65), &CancelCause::Closed).unwrap();
    let (cancelled, out) = close();
    assert!(out.is_empty(), "{out:?}");
    let box_id = cancelled.and_then(|c| c.state.box_id()).unwrap();
    let run = r.run_for(id, 65, OLD);
    assert!(
        matches!(
            run.state,
            RunState::Done(RunEnd::Ended {
                outcome: RunOutcome::Cancelled {
                    cause: CancelCause::Closed,
                    ..
                },
                ..
            })
        ),
        "{:?}",
        run.state
    );
    assert_eq!(r.session(box_id), SessionState::Ended);
    assert_eq!(close().0, None, "a second close finds nothing open");
    assert!(matches!(
        r.run_for(id, 66, OLD).state,
        RunState::Running { .. }
    ));
}

#[test]
fn a_pull_requests_run_closes_only_that_pull_requests_earlier_boxes() {
    let r = rig_probing("ahead=0 dirty=0\n");
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(5_000);
    started(&r.fire_pull("daily-fix", 65, OLD));
    let first = r.latest_box(id);
    r.cx.store()
        .unwrap()
        .apply(Decision::new(first).session(SessionInput::AgentQuiet))
        .unwrap();
    r.advance_to(5_000 + 3 * 60_000);
    r.pass();
    assert!(matches!(
        r.run_for(id, 65, OLD).state,
        RunState::Done(RunEnd::Ended { .. })
    ));
    put_to_sleep(&r, first);

    started(&r.fire_pull("daily-fix", 66, OLD));
    assert_eq!(
        lifecycle(&r, first),
        BoxLifecycle::Asleep,
        "another pull request's run leaves it be"
    );
    r.pause(1_000);
    started(&r.fire_pull("daily-fix", 65, NEW));
    assert_eq!(
        lifecycle(&r, first),
        BoxLifecycle::Closed,
        "the same pull request's next run closes it"
    );
}

fn ask_then_ship() -> Plan {
    let steps = json!({ "steps": [
        {"id": "ask", "title": "Ask", "kind": "approve", "question": "ship it?", "ok": "ship"},
        {"id": "ship", "title": "Ship", "kind": "deliver", "instructions": "ship it",
         "done_when": "shipped"},
    ]});
    Plan::parse(&steps.to_string(), PROMPT).unwrap()
}

#[test]
fn with_two_runs_waiting_an_answer_names_its_pull_request() {
    let r = rig();
    r.answer_repo();
    let id = r.put_planned(Some(ask_then_ship()));
    r.advance_to(5_000);
    started(&r.fire_pull("deps", 65, OLD));
    started(&r.fire_pull("deps", 66, OLD));
    assert!(r.pass().is_empty());
    r.pause(60_000);
    assert!(r.pass().is_empty());
    let waits = |n| matches!(r.run_for(id, n, OLD).state, RunState::Waiting { .. });
    assert!(waits(65) && waits(66));

    let store = r.cx.store().unwrap();
    let answer = |pull: Option<&PullTarget>| {
        answer_run(
            &r.cx,
            store,
            REQUESTER_USER,
            ("deps", pull),
            (Answer::Yes, None),
        )
    };
    let why = answer(None).unwrap_err().error.to_string();
    assert!(why.contains("2 runs of deps wait"), "{why}");
    assert!(why.contains("o/n#65") && why.contains("o/n#66"), "{why}");
    assert!(waits(65) && waits(66), "a refused answer moves nothing");

    assert!(answer(Some(&target(66))).is_ok());
    assert!(!waits(66));
    assert!(waits(65));
    let why = answer(Some(&target(67))).unwrap_err().error.to_string();
    assert!(why.contains("no run of deps for o/n#67"), "{why}");
    assert!(answer(None).is_ok(), "the one still waiting needs no name");
    assert!(!waits(65));
}
