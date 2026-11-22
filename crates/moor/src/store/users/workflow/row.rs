//! How a `workflow_runs` row reads as a [`WorkflowRun`] and a [`RunState`] is stored as columns.

use moor_core::gate::Verdict;
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    CancelCause, CommitSha, Plan, PullRef, RunCause, RunEnd, RunId, RunOutcome, RunState,
    RunTrigger, StepId, Unfinished, WorkflowId, WorkflowRun,
};
use moor_core::BoxId;
use moor_schema::tables::WorkflowRuns;

use super::corrupt;
use crate::store::db;
use crate::store::StoreError;

/// The `outcome` of a run claimed before its Box exists.
pub(super) const STARTING: &str = "starting";
/// The `outcome` of a run that started a Box.
pub(super) const STARTED: &str = "started";
/// The `outcome` of a run that waits for its owner.
pub(super) const WAITING: &str = "waiting";
/// The `outcome` of a run whose agent delivered, or went quiet at the last step.
pub(super) const FINISHED: &str = "finished";
/// The `outcome` of a run that failed at a step.
const FAILED: &str = "failed";
/// The `outcome` of a run whose agent stopped without reporting a step.
const INCOMPLETE: &str = "incomplete";
/// The `outcome` of a run its owner declined.
const DECLINED: &str = "declined";
/// The `outcome` of a run expired at its timeout.
const TIMED_OUT: &str = "timed_out";
/// The `outcome` of a run skipped for an open previous Box.
pub(super) const SKIPPED_OVERLAP: &str = "skipped_overlap";
/// The `outcome` of a pull request run skipped because its fork's author may not write.
pub(super) const SKIPPED_FORK: &str = "skipped_fork";
/// The `outcome` of a run whose start was refused.
pub(super) const REFUSED: &str = "refused";
/// The `outcome` of a pull request run a newer head or a close cancelled.
const CANCELLED: &str = "cancelled";
/// The `detail` of a run cancelled by its pull request's close; a newer head stores its SHA.
const CLOSED: &str = "closed";
/// The `detail` of a run left incomplete because its Box was closed.
const BOX_CLOSED: &str = "box_closed";

/// A `workflow_runs` row as read.
pub(super) struct RunRow {
    /// `id`.
    id: i64,
    /// `repo`.
    repo: String,
    /// `due_at`.
    due_at: i64,
    /// `box`.
    pub(super) box_id: Option<i64>,
    /// `outcome`.
    outcome: String,
    /// `detail`.
    pub(super) detail: Option<String>,
    /// `ended_at`.
    pub(super) ended_at: Option<i64>,
    /// `recorded_at`.
    recorded_at: i64,
    /// `step`.
    pub(super) step: Option<String>,
    /// `try_n`.
    pub(super) try_n: Option<i64>,
    /// `waiting_since`.
    pub(super) waiting_since: Option<i64>,
    /// `ended_step`.
    pub(super) ended_step: Option<String>,
    /// `trigger`.
    trigger: String,
    /// `pull_number`.
    pull_number: Option<i64>,
    /// `head_sha`.
    head_sha: Option<String>,
    /// `verdict`.
    verdict: Option<String>,
}

/// The columns of a `workflow_runs` row, in the order [`read_run`] reads them.
pub(super) const RUN_COLS: [WorkflowRuns; 16] = [
    WorkflowRuns::Id,
    WorkflowRuns::Repo,
    WorkflowRuns::DueAt,
    WorkflowRuns::Box,
    WorkflowRuns::Outcome,
    WorkflowRuns::Detail,
    WorkflowRuns::EndedAt,
    WorkflowRuns::RecordedAt,
    WorkflowRuns::Step,
    WorkflowRuns::TryN,
    WorkflowRuns::WaitingSince,
    WorkflowRuns::EndedStep,
    WorkflowRuns::Trigger,
    WorkflowRuns::PullNumber,
    WorkflowRuns::HeadSha,
    WorkflowRuns::Verdict,
];

/// Reads one `workflow_runs` row whose columns start at `first`.
pub(super) fn read_run(r: &db::Row, first: usize) -> Result<RunRow, db::Error> {
    Ok(RunRow {
        id: r.get(first)?,
        repo: r.get(first + 1)?,
        due_at: r.get(first + 2)?,
        box_id: r.get(first + 3)?,
        outcome: r.get(first + 4)?,
        detail: r.get(first + 5)?,
        ended_at: r.get(first + 6)?,
        recorded_at: r.get(first + 7)?,
        step: r.get(first + 8)?,
        try_n: r.get(first + 9)?,
        waiting_since: r.get(first + 10)?,
        ended_step: r.get(first + 11)?,
        trigger: r.get(first + 12)?,
        pull_number: r.get(first + 13)?,
        head_sha: r.get(first + 14)?,
        verdict: r.get(first + 15)?,
    })
}

/// The outcome a delivered run of a Box with gates keeps; none for any other run.
fn verdict_of(row: &RunRow) -> Result<Option<Verdict>, StoreError> {
    row.verdict
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| corrupt("run verdict", e))
}

/// The step stored in `col`; a run from before plans reads at the only step of
/// [`Plan::single`].
fn step_of(step: Option<String>) -> Result<StepId, StoreError> {
    step.map_or_else(
        || Ok(Plan::single_id()),
        |s| StepId::parse(&s).map_err(|e| corrupt("run step", e)),
    )
}

/// The try stored with a step; a run from before plans is on its first.
fn try_of(try_n: Option<i64>) -> Result<u8, StoreError> {
    try_n.map_or(Ok(1), |n| {
        u8::try_from(n).map_err(|e| corrupt("run try", e))
    })
}

/// The run a stored row holds; a state whose box, detail or end do not fit it is corrupt.
pub(super) fn run_of(workflow: WorkflowId, row: &RunRow) -> Result<WorkflowRun, StoreError> {
    let repo = ForgeRepo::parse(&row.repo).map_err(|e| corrupt("run repo", e))?;
    Ok(WorkflowRun {
        id: RunId::new(row.id),
        workflow,
        cause: cause_of(row, &repo)?,
        repo,
        due_at: Ts::from_millis(row.due_at),
        state: state_of(row)?,
        recorded_at: Ts::from_millis(row.recorded_at),
    })
}

/// The state a row's `outcome` and its columns stand for; a state whose box, detail or end do not
/// fit it is corrupt.
fn state_of(row: &RunRow) -> Result<RunState, StoreError> {
    let box_id = row.box_id.map(BoxId::new);
    let ended_at = row.ended_at.map(Ts::from_millis);
    let bad = || corrupt("run outcome", &row.outcome);
    let need_box = || box_id.ok_or_else(bad);
    let need_end = || ended_at.ok_or_else(bad);
    let ended_step = |row: &RunRow| step_of(row.ended_step.clone());
    let ended = |outcome| -> Result<RunState, StoreError> {
        Ok(RunState::Done(RunEnd::Ended {
            box_id: need_box()?,
            ended_at: need_end()?,
            outcome,
        }))
    };
    let state = match row.outcome.as_str() {
        STARTING => RunState::Queued,
        STARTED => RunState::Running {
            box_id: need_box()?,
            at: step_of(row.step.clone())?,
            try_n: try_of(row.try_n)?,
        },
        WAITING => RunState::Waiting {
            box_id: need_box()?,
            at: step_of(row.step.clone())?,
            try_n: try_of(row.try_n)?,
            since: row.waiting_since.map(Ts::from_millis).ok_or_else(bad)?,
        },
        FINISHED => ended(RunOutcome::Delivered {
            result: row.detail.clone().unwrap_or_default(),
            verdict: verdict_of(row)?,
        })?,
        FAILED => ended(RunOutcome::Failed {
            at: ended_step(row)?,
            reason: row.detail.clone().unwrap_or_default(),
        })?,
        INCOMPLETE => ended(incomplete_of(row)?)?,
        DECLINED => ended(RunOutcome::Declined {
            at: ended_step(row)?,
        })?,
        TIMED_OUT => ended(RunOutcome::TimedOut {
            at: ended_step(row)?,
        })?,
        CANCELLED if box_id.is_none() => RunState::Done(RunEnd::CancelledUnstarted {
            cause: row.detail.as_deref().and_then(cancel_of).ok_or_else(bad)?,
        }),
        CANCELLED => ended(RunOutcome::Cancelled {
            at: ended_step(row)?,
            cause: row.detail.as_deref().and_then(cancel_of).ok_or_else(bad)?,
        })?,
        SKIPPED_OVERLAP => RunState::Done(RunEnd::SkippedOverlap),
        SKIPPED_FORK => RunState::Done(RunEnd::SkippedFork {
            author: row.detail.clone().ok_or_else(bad)?,
        }),
        REFUSED => RunState::Done(RunEnd::Refused {
            detail: row.detail.clone().ok_or_else(bad)?,
        }),
        _ => return Err(bad()),
    };
    Ok(state)
}

/// The cancel cause a `detail` holds: `closed`, or the newer head's SHA.
fn cancel_of(detail: &str) -> Option<CancelCause> {
    if detail == CLOSED {
        return Some(CancelCause::Closed);
    }
    CommitSha::parse(detail)
        .ok()
        .map(|sha| CancelCause::NewerHead { sha })
}

/// The outcome of an incomplete run's row; a `detail` no cause is stored as is corrupt.
fn incomplete_of(row: &RunRow) -> Result<RunOutcome, StoreError> {
    let why = match row.detail.as_deref() {
        None => Unfinished::Unreported,
        Some(BOX_CLOSED) => Unfinished::BoxClosed,
        Some(other) => return Err(corrupt("incomplete run detail", other)),
    };
    Ok(RunOutcome::Incomplete {
        at: step_of(row.ended_step.clone())?,
        why,
    })
}

/// The `detail` a cancel cause is stored as: `closed`, or the newer head's SHA.
fn cancel_detail(cause: &CancelCause) -> String {
    match cause {
        CancelCause::NewerHead { sha } => sha.to_string(),
        CancelCause::Closed => CLOSED.to_owned(),
    }
}

/// The cause a row's `trigger`, `pull_number` and `head_sha` stand for; a trigger that does not
/// fit its pull request columns is corrupt.
fn cause_of(row: &RunRow, repo: &ForgeRepo) -> Result<RunCause, StoreError> {
    let bad = || corrupt("run trigger", &row.trigger);
    let trigger = RunTrigger::from_name(&row.trigger).ok_or_else(bad)?;
    let pull = match (row.pull_number, row.head_sha.as_deref()) {
        (None, None) => None,
        (Some(number), Some(sha)) => Some(PullRef {
            repo: repo.clone(),
            number: u64::try_from(number).map_err(|e| corrupt("run pull request", e))?,
            head_sha: CommitSha::parse(sha).map_err(|e| corrupt("run head", e))?,
        }),
        _ => return Err(bad()),
    };
    RunCause::from_parts(trigger, pull).ok_or_else(bad)
}

/// The `pull_number` and `head_sha` a run's cause is stored in.
pub(super) fn subject_cols(cause: &RunCause) -> (Option<i64>, Option<String>) {
    cause.subject().map_or((None, None), |pull| {
        (
            i64::try_from(pull.number).ok(),
            Some(pull.head_sha.to_string()),
        )
    })
}

/// The columns a state is stored in; a column the state has no use for is null.
pub(super) struct StateCols {
    /// `box`.
    pub(super) box_id: Option<i64>,
    /// `detail`: a refusal's reason, a failure's reason, a delivery's result, a cancel's cause.
    pub(super) detail: Option<String>,
    /// `ended_at`.
    pub(super) ended_at: Option<i64>,
    /// `step`.
    pub(super) step: Option<String>,
    /// `try_n`.
    pub(super) try_n: Option<i64>,
    /// `waiting_since`.
    pub(super) waiting_since: Option<i64>,
    /// `ended_step`.
    pub(super) ended_step: Option<String>,
    /// `verdict`: the outcome of a delivered run of a Box with gates, as JSON.
    pub(super) verdict: Option<String>,
}

/// `state` as stored; its `outcome` is [`RunState::tag`].
pub(super) fn state_cols(state: &RunState) -> StateCols {
    let mut cols = StateCols {
        box_id: state.box_id().map(BoxId::get),
        detail: None,
        ended_at: None,
        step: None,
        try_n: None,
        waiting_since: None,
        ended_step: None,
        verdict: None,
    };
    match state {
        RunState::Queued | RunState::Done(RunEnd::SkippedOverlap) => {}
        RunState::Running { at, try_n, .. } => {
            cols.step = Some(at.to_string());
            cols.try_n = Some(i64::from(*try_n));
        }
        RunState::Waiting {
            at, try_n, since, ..
        } => {
            cols.step = Some(at.to_string());
            cols.try_n = Some(i64::from(*try_n));
            cols.waiting_since = Some(since.as_millis());
        }
        RunState::Done(RunEnd::Refused { detail }) => cols.detail = Some(detail.clone()),
        RunState::Done(RunEnd::SkippedFork { author }) => cols.detail = Some(author.clone()),
        RunState::Done(RunEnd::CancelledUnstarted { cause }) => {
            cols.detail = Some(cancel_detail(cause));
        }
        RunState::Done(RunEnd::Ended {
            ended_at, outcome, ..
        }) => {
            cols.ended_at = Some(ended_at.as_millis());
            match outcome {
                RunOutcome::Delivered { result, verdict } => {
                    cols.detail = Some(result.clone()).filter(|r| !r.is_empty());
                    cols.verdict = verdict.as_ref().and_then(|v| serde_json::to_string(v).ok());
                }
                RunOutcome::Failed { at, reason } => {
                    cols.detail = Some(reason.clone());
                    cols.ended_step = Some(at.to_string());
                }
                RunOutcome::Incomplete { at, why } => {
                    cols.detail = (why == &Unfinished::BoxClosed).then(|| BOX_CLOSED.to_owned());
                    cols.ended_step = Some(at.to_string());
                }
                RunOutcome::Declined { at } | RunOutcome::TimedOut { at } => {
                    cols.ended_step = Some(at.to_string());
                }
                RunOutcome::Cancelled { at, cause } => {
                    cols.detail = Some(cancel_detail(cause));
                    cols.ended_step = Some(at.to_string());
                }
            }
        }
    }
    cols
}
