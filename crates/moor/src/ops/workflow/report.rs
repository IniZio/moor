//! `step-report`: what the agent in a Box reports for the step its run is at.
//!
//! The reply always says what to do next. A report that passes the code gate moves the run on, or
//! ends it, in one transaction with its lines, compare-and-set on the run's state, step and try,
//! so a report repeated or raced by a tick applies once.

use moor_core::artifact::ArtifactFile;
use moor_core::decide::workflow::{code_gate, refusal_move, report_move, Move};
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::view::workflow::StepReportValue;
use moor_core::workflow::{
    HistoryEntry, Plan, RunOutcome, RunState, Step, StepId, StepKind, StepReport, StepResult,
    Workflow, WorkflowId, WorkflowRun,
};
use moor_core::BoxId;

use super::facts::history_of;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::artifacts::index_now;
use crate::store::Store;

/// How often one report reads the run again after a tick moved it first.
const TRIES: usize = 3;

/// The reply for a Box with no run at a step.
const NO_RUN: &str = "This Box has no running workflow run, so there is no step to report. \
                      Nothing is recorded; carry on with what you were asked.";

/// The reply for a step moor runs itself.
const END_TURN_CHECK: &str = "End your turn. moor runs the check and sends you the result.";

/// The reply for a step the owner answers.
const END_TURN_APPROVE: &str = "End your turn. moor asks the owner and sends you the answer.";

/// A reply that tells the agent how to go on.
fn said(reply: impl Into<String>) -> StepReportValue {
    StepReportValue {
        reply: reply.into(),
        error: false,
    }
}

/// A reply that tells the agent why nothing moved.
fn refused(reply: impl Into<String>) -> StepReportValue {
    StepReportValue {
        reply: reply.into(),
        error: true,
    }
}

/// What the agent is told about `step` when it is the one to do next.
fn instructions(step: &Step) -> String {
    match &step.kind {
        StepKind::Work { .. } | StepKind::Deliver { .. } => Plan::next_step_steer(step),
        StepKind::Check { .. } => END_TURN_CHECK.to_owned(),
        StepKind::Approve { .. } => END_TURN_APPROVE.to_owned(),
    }
}

/// Where a run stands for one report: what the store says, read together.
struct Standing {
    /// The workflow.
    w: Workflow,
    /// The run.
    run: WorkflowRun,
    /// The frozen plan.
    plan: Plan,
    /// The step the run is at, and its try.
    at: (StepId, u8),
    /// Whether the run waits at that step for the owner's answer.
    waiting: bool,
    /// The run's lines, with when the agent was last told something.
    history: Vec<HistoryEntry>,
}

/// Reads the run at a step in Box `id`; none when the Box has none.
fn standing(store: &Store, id: BoxId) -> Result<Option<Standing>, CommandError> {
    let Some((wid, run)) = store.running_run_of_box(id)? else {
        return Ok(None);
    };
    let (RunState::Running { ref at, try_n, .. } | RunState::Waiting { ref at, try_n, .. }) =
        run.state
    else {
        return Ok(None);
    };
    let waiting = matches!(run.state, RunState::Waiting { .. });
    let at = (at.clone(), try_n);
    let Some(w) = store.workflow_by_id(wid)? else {
        return Ok(None);
    };
    let plan = store.run_plan(run.id, &w.prompt)?;
    let (history, _) = history_of(store, run.id, (&at.0, at.1))?;
    Ok(Some(Standing {
        w,
        run,
        plan,
        at,
        waiting,
        history,
    }))
}

/// `step-report`: records what the agent in `req.box_id` reported and tells it what to do next.
pub(crate) fn run(
    cx: &Cx,
    req: &moor_core::request::StepReportReq,
) -> Result<StepReportValue, CommandError> {
    let store = cx.store()?;
    for _ in 0..TRIES {
        let Some(s) = standing(store, req.box_id)? else {
            return Ok(refused(NO_RUN));
        };
        #[cfg(test)]
        tests_hook::after_standing(cx);
        if let Some(reply) = settle(cx, store, &s, &req.report)? {
            return Ok(reply);
        }
    }
    Ok(refused(
        "The run moved while you reported. Call report_step again if the step is still yours.",
    ))
}

/// Handles `report` against `s`; none when the run moved under it and the caller reads again.
fn settle(
    cx: &Cx,
    store: &Store,
    s: &Standing,
    report: &StepReport,
) -> Result<Option<StepReportValue>, CommandError> {
    let Some(current) = s.plan.step(&s.at.0) else {
        return Ok(Some(refused("The run's plan has no such step.")));
    };
    if s.waiting {
        return Ok(Some(while_waiting(s, report)));
    }
    if report.step != s.at.0 {
        return Ok(Some(elsewhere(s, current, report)));
    }
    if !matches!(
        current.kind,
        StepKind::Work { .. } | StepKind::Deliver { .. }
    ) {
        return Ok(Some(refused(format!(
            "Step {} is not yours to report. {}",
            current.id,
            instructions(current)
        ))));
    }
    let index = match evidence_index(cx, store, s, current, report)? {
        Ok(index) => index,
        Err(reply) => return Ok(Some(reply)),
    };
    let (step, try_n) = s.at.clone();
    let Err(problems) = code_gate(current, report, &index) else {
        let lines = vec![HistoryEntry::Reported {
            step,
            try_n,
            report: report.clone(),
        }];
        let m = report_move(&s.plan, &s.history, report);
        return apply(cx, store, s, (m, lines), &report.note);
    };
    let list = problems.join("; ");
    let lines = vec![HistoryEntry::GateRefused {
        step,
        try_n,
        missing: problems,
    }];
    match refusal_move(&s.plan, &s.history) {
        Move::Wait => {
            let key = (s.w.id, &s.run.repo, s.run.due_at);
            let at = (&s.at.0, s.at.1);
            if !store.step_workflow_run(key, (at, at), (&lines, cx.clock().now()))? {
                return Ok(None);
            }
            Ok(Some(refused(format!(
                "Refused: {list}. Fix this and call report_step again. A second refusal fails \
                 this step."
            ))))
        }
        m => apply(cx, store, s, (m, lines), &list),
    }
}

/// The files the gate checks a report of `current` against, looked up in the Box now; a work
/// step's ok report is the only one that needs them. The reply when the Box cannot be looked in.
fn evidence_index(
    cx: &Cx,
    store: &Store,
    s: &Standing,
    current: &Step,
    report: &StepReport,
) -> Result<Result<Vec<ArtifactFile>, StepReportValue>, CommandError> {
    if !matches!(current.kind, StepKind::Work { .. }) || report.result != StepResult::Ok {
        return Ok(Ok(Vec::new()));
    }
    let Some(row) = s
        .run
        .state
        .box_id()
        .map(|id| store.box_row(id))
        .transpose()?
        .flatten()
    else {
        return Ok(Err(refused(NO_RUN)));
    };
    Ok(index_now(cx, &row).map_err(|why| {
        refused(format!(
            "moor could not look in the Box to check your evidence ({why}). Nothing is \
             recorded; call report_step again."
        ))
    }))
}

/// The reply to a report while the run waits for the owner: the agent is to end its turn, not
/// carry on. The duplicate of the report that entered the wait, sent again after a lost reply, is
/// already recorded.
fn while_waiting(s: &Standing, report: &StepReport) -> StepReportValue {
    if last_reported(s) == Some(report) {
        return said(format!("Already recorded. {END_TURN_APPROVE}"));
    }
    refused(format!(
        "The run waits for the owner's answer at step {}, so there is no step to report. \
         {END_TURN_APPROVE}",
        s.at.0
    ))
}

/// The last report in the run's lines.
fn last_reported(s: &Standing) -> Option<&StepReport> {
    s.history.iter().rev().find_map(|e| match e {
        HistoryEntry::Reported { report, .. } => Some(report),
        _ => None,
    })
}

/// The reply to a report of a step that is not the current one: the duplicate of the last
/// report, or the current step again.
fn elsewhere(s: &Standing, current: &Step, report: &StepReport) -> StepReportValue {
    if last_reported(s) == Some(report) {
        return said(format!("Already recorded. {}", instructions(current)));
    }
    refused(format!(
        "Step {} is not the current step. The current step is {}. {}",
        report.step,
        current.id,
        instructions(current)
    ))
}

/// The run a report moves, and the move's key in the store.
struct Moving<'a> {
    /// The store.
    store: &'a Store,
    /// Where the run stands.
    s: &'a Standing,
    /// The Box the run is in.
    box_id: BoxId,
    /// Now.
    now: Ts,
}

impl Moving<'_> {
    /// The run's key.
    const fn key(&self) -> (WorkflowId, &ForgeRepo, Ts) {
        (self.s.w.id, &self.s.run.repo, self.s.run.due_at)
    }

    /// Ends the run with `outcome`, with `lines` written in the same move; false when the run
    /// moved under it.
    fn end(&self, lines: &[HistoryEntry], outcome: RunOutcome) -> Result<bool, CommandError> {
        let at = (&self.s.at.0, self.s.at.1);
        Ok(self.store.end_workflow_run(
            self.key(),
            at,
            (self.box_id, outcome),
            (lines, self.now),
        )?)
    }

    /// Goes on to `step`, for its `try_n`th entry, with `lines` written in the same move; what
    /// the agent is told, or none when the run moved under it.
    fn enter(
        &self,
        (step, try_n): (StepId, u8),
        mut lines: Vec<HistoryEntry>,
    ) -> Result<Option<StepReportValue>, CommandError> {
        let Some(next) = self.s.plan.step(&step) else {
            return Ok(Some(refused("The run's plan has no such step.")));
        };
        lines.push(HistoryEntry::Entered {
            step: step.clone(),
            try_n,
        });
        if matches!(next.kind, StepKind::Work { .. } | StepKind::Deliver { .. }) {
            lines.push(HistoryEntry::Sent {
                step: step.clone(),
                try_n,
            });
        }
        let at = (&self.s.at.0, self.s.at.1);
        let moved = if matches!(next.kind, StepKind::Approve { .. }) {
            self.store.wait_workflow_run(
                self.key(),
                (at, (&step, try_n)),
                self.box_id,
                (&lines, self.now),
            )?
        } else {
            self.store
                .step_workflow_run(self.key(), (at, (&step, try_n)), (&lines, self.now))?
        };
        Ok(moved.then(|| said(instructions(next))))
    }
}

/// The lines for steps a move passed over for lack of tries.
fn exhausted(steps: Vec<StepId>) -> impl Iterator<Item = HistoryEntry> {
    steps
        .into_iter()
        .map(|step| HistoryEntry::Exhausted { step })
}

/// Makes `m`, the move a report decided, with `lines` written in the same move. `note` is a
/// delivered run's result. None when the run moved under it.
fn apply(
    cx: &Cx,
    store: &Store,
    s: &Standing,
    (m, mut lines): (Move, Vec<HistoryEntry>),
    note: &str,
) -> Result<Option<StepReportValue>, CommandError> {
    let Some(box_id) = s.run.state.box_id() else {
        return Ok(Some(refused(NO_RUN)));
    };
    let moving = Moving {
        store,
        s,
        box_id,
        now: cx.clock().now(),
    };
    match m {
        Move::Enter {
            step,
            try_n,
            exhausted: passed,
        } => {
            lines.extend(exhausted(passed));
            moving.enter((step, try_n), lines)
        }
        Move::Delivered => {
            let result = note.trim().to_owned();
            let done = moving.end(&lines, RunOutcome::Delivered { result })?;
            Ok(done.then(|| said("Done.")))
        }
        Move::Fail {
            at,
            reason,
            exhausted: passed,
        } => {
            lines.extend(exhausted(passed));
            let outcome = RunOutcome::Failed {
                at,
                reason: reason.clone(),
            };
            let done = moving.end(&lines, outcome)?;
            Ok(done.then(|| {
                said(format!(
                    "The run ended: {reason}. There is nothing more to do."
                ))
            }))
        }
        Move::Wait
        | Move::Remind
        | Move::RunCheck
        | Move::Ask
        | Move::Declined { .. }
        | Move::Incomplete { .. } => Ok(Some(refused(
            "The run could not move on from this report. Nothing more is recorded.",
        ))),
    }
}

#[cfg(test)]
pub(super) mod tests_hook;
