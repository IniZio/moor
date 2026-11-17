//! `step-report`: what the agent in a Box reports for the step its run is at.
//!
//! The reply always says what to do next. A report that passes the code gate moves the run on in
//! one compare-and-set, with its lines, so a report repeated or raced by a tick applies once.

use moor_core::artifact::ArtifactFile;
use moor_core::decide::workflow::{code_gate, refusal_move, report_move, Move};
use moor_core::view::workflow::StepReportValue;
use moor_core::workflow::{
    HistoryEntry, Plan, RunOutcome, RunState, Step, StepId, StepKind, StepReport, StepResult,
    Workflow, WorkflowRun,
};
use moor_core::BoxId;

use super::advance::history_of;
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
    /// The run's lines, with when the agent was last told something.
    history: Vec<HistoryEntry>,
}

/// Reads the run at a step in Box `id`; none when the Box has none.
fn standing(store: &Store, id: BoxId) -> Result<Option<Standing>, CommandError> {
    let Some((wid, run)) = store.running_run_of_box(id)? else {
        return Ok(None);
    };
    let RunState::Running { ref at, try_n, .. } = run.state else {
        return Ok(None);
    };
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

/// The reply to a report of a step that is not the current one: the duplicate of the last
/// report, or the current step again.
fn elsewhere(s: &Standing, current: &Step, report: &StepReport) -> StepReportValue {
    let last = s.history.iter().rev().find_map(|e| match e {
        HistoryEntry::Reported { report, .. } => Some(report),
        _ => None,
    });
    if last == Some(report) {
        return said(format!("Already recorded. {}", instructions(current)));
    }
    refused(format!(
        "Step {} is not the current step. The current step is {}. {}",
        report.step,
        current.id,
        instructions(current)
    ))
}

/// Makes `m`, the move a report decided, with `lines` written in the same move. `note` is a
/// delivered run's result and a failed one's reason. None when the run moved under it.
fn apply(
    cx: &Cx,
    store: &Store,
    s: &Standing,
    (m, mut lines): (Move, Vec<HistoryEntry>),
    note: &str,
) -> Result<Option<StepReportValue>, CommandError> {
    let now = cx.clock().now();
    let key = (s.w.id, &s.run.repo, s.run.due_at);
    let at = (&s.at.0, s.at.1);
    let Some(box_id) = s.run.state.box_id() else {
        return Ok(Some(refused(NO_RUN)));
    };
    let exhausted = |steps: Vec<StepId>| {
        steps
            .into_iter()
            .map(|step| HistoryEntry::Exhausted { step })
    };
    match m {
        Move::Enter {
            step,
            try_n,
            exhausted: passed,
        } => {
            let Some(next) = s.plan.step(&step) else {
                return Ok(Some(refused("The run's plan has no such step.")));
            };
            lines.extend(exhausted(passed));
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
            if !store.step_workflow_run(key, (at, (&step, try_n)), (&lines, now))? {
                return Ok(None);
            }
            Ok(Some(said(instructions(next))))
        }
        Move::Delivered => {
            if !store.step_workflow_run(key, (at, at), (&lines, now))? {
                return Ok(None);
            }
            let outcome = RunOutcome::Delivered {
                result: note.trim().to_owned(),
            };
            if !store.end_workflow_run(key, (box_id, outcome), now)? {
                return Ok(None);
            }
            Ok(Some(said("Done.")))
        }
        Move::Fail {
            at: failed,
            reason,
            exhausted: passed,
        } => {
            lines.extend(exhausted(passed));
            if !store.step_workflow_run(key, (at, at), (&lines, now))? {
                return Ok(None);
            }
            let outcome = RunOutcome::Failed {
                at: failed,
                reason: reason.clone(),
            };
            if !store.end_workflow_run(key, (box_id, outcome), now)? {
                return Ok(None);
            }
            Ok(Some(said(format!(
                "The run ended: {reason}. There is nothing more to do."
            ))))
        }
        Move::Wait | Move::Remind | Move::RunCheck | Move::Incomplete { .. } => Ok(Some(refused(
            "The run could not move on from this report. Nothing more is recorded.",
        ))),
    }
}
