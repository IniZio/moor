//! The workflow tick decider: which repos of a workflow have a firing due.

use crate::name::{BoxId, ForgeRepo};
use crate::seam::clock::Ts;
use crate::workflow::{
    RunEnd, RunId, RunOutcome, RunState, StepId, Workflow, WorkflowRun, WorkflowState,
    BREAKER_STREAK,
};

/// What the store and the Boxes say about one repo of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRepoFacts {
    /// The repo.
    pub repo: ForgeRepo,
    /// The due time of the latest recorded firing, whatever came of it.
    pub last_due: Option<Ts>,
    /// The latest run that was a real attempt, looking through skipped and refused ones.
    pub prior: Option<PriorRun>,
}

/// A run that may keep the next firing of its repo from starting a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorRun {
    /// Its Box is being started.
    Starting,
    /// Its Box was started and has not ended.
    Started,
    /// Its agent finished.
    Finished,
    /// It ran to its timeout.
    TimedOut {
        /// Whether its Box, which the timeout keeps, is still open.
        box_open: bool,
    },
}

impl PriorRun {
    /// Whether this run keeps the next firing from starting: while it is starting or started,
    /// or after a timeout while its Box is open. A finished run never does, whatever its Box.
    const fn blocks(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Started | Self::TimedOut { box_open: true }
        )
    }
}

/// What to do for one repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowVerdict {
    /// Nothing is due.
    NotDue,
    /// Start a Box and record it against `due_at`.
    Start {
        /// The scheduled time this firing answers.
        due_at: Ts,
    },
    /// Start nothing and record that the previous Box overlaps `due_at`.
    SkipOverlap {
        /// The scheduled time this firing answers.
        due_at: Ts,
    },
}

/// The verdict for one repo of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRepoVerdict {
    /// The repo.
    pub repo: ForgeRepo,
    /// What to do.
    pub verdict: WorkflowVerdict,
}

/// Decides, per repo, whether `workflow` has a firing due at `now`.
///
/// Only the latest due time at or before `now` fires, and only once: a workflow that missed
/// several times while the controller was down fires the newest and never a backlog. A due time
/// before the workflow was created or last enabled never fires. Repos in `facts` the workflow
/// does not list are ignored, and a repo with no facts is treated as never fired and not open.
#[must_use]
pub fn workflow_due(
    workflow: &Workflow,
    facts: &[WorkflowRepoFacts],
    now: Ts,
) -> Vec<WorkflowRepoVerdict> {
    let WorkflowState::Enabled { since } = workflow.state else {
        return Vec::new();
    };
    let latest = workflow.schedule.latest_at_or_before(now);
    workflow
        .repos
        .iter()
        .map(|repo| {
            let fact = facts.iter().find(|f| &f.repo == repo);
            let last_due = fact.and_then(|f| f.last_due);
            let prior = fact.and_then(|f| f.prior);
            let due = latest.filter(|&d| d >= since && last_due.is_none_or(|l| d > l));
            WorkflowRepoVerdict {
                repo: repo.clone(),
                verdict: due.map_or(WorkflowVerdict::NotDue, |due_at| {
                    overlap_policy(prior, due_at)
                }),
            }
        })
        .collect()
}

/// A started run that has outlived its timeout, and the Box to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowExpiry {
    /// The repo the run is for.
    pub repo: ForgeRepo,
    /// The due time the run answers.
    pub due_at: Ts,
    /// The Box to stop.
    pub box_id: BoxId,
    /// The step the run was at.
    pub at: StepId,
    /// The try of that step.
    pub try_n: u8,
}

/// How long a run may take from its claim until its agent has started, queueing and setup
/// included. A run that has not started its agent by then is refused and its Box closed.
pub const START_LIMIT: std::time::Duration = std::time::Duration::from_hours(1);

/// A run that did not start its agent within [`START_LIMIT`] of its claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartBreach {
    /// The repo the run is for.
    pub repo: ForgeRepo,
    /// The due time the run answers.
    pub due_at: Ts,
    /// The Box the run recorded, none while it is still being started.
    pub box_id: Option<BoxId>,
    /// Whether the run only claimed its firing so far.
    pub claimed_only: bool,
}

/// The runs in `open` whose agent has not started [`START_LIMIT`] after the run was claimed.
/// `agent_started` tells when a Box's agent first started, if it did.
#[must_use]
pub fn workflow_start_overdue(
    open: &[WorkflowRun],
    agent_started: &dyn Fn(BoxId) -> Option<Ts>,
    now: Ts,
) -> Vec<StartBreach> {
    let limit = i64::try_from(START_LIMIT.as_millis()).unwrap_or(i64::MAX);
    open.iter()
        .filter(|run| now.as_millis().saturating_sub(run.recorded_at.as_millis()) >= limit)
        .filter_map(|run| {
            let (box_id, claimed_only) = match run.state {
                RunState::Queued => (None, true),
                RunState::Running { box_id, .. } if agent_started(box_id).is_none() => {
                    (Some(box_id), false)
                }
                _ => return None,
            };
            Some(StartBreach {
                repo: run.repo.clone(),
                due_at: run.due_at,
                box_id,
                claimed_only,
            })
        })
        .collect()
}

/// The runs in `open` that have worked for the workflow's timeout.
///
/// The time counts on the wall clock from when the run's agent first started, less the
/// milliseconds `waited` says the run spent waiting for answers. A run whose agent has not
/// started is never expired here; the start limit covers it. A run that waits is never expired
/// here either: its clock is stopped, and [`workflow_unanswered`] ends it after a week.
#[must_use]
pub fn workflow_expired(
    workflow: &Workflow,
    open: &[WorkflowRun],
    (agent_started, waited): (&dyn Fn(BoxId) -> Option<Ts>, &dyn Fn(RunId) -> i64),
    now: Ts,
) -> Vec<WorkflowExpiry> {
    let limit = workflow.timeout.millis();
    open.iter()
        .filter_map(|run| match &run.state {
            RunState::Running { box_id, at, try_n }
                if agent_started(*box_id).is_some_and(|began| {
                    let spent = now.as_millis().saturating_sub(began.as_millis());
                    spent >= limit && spent.saturating_sub(waited(run.id)) >= limit
                }) =>
            {
                Some(WorkflowExpiry {
                    repo: run.repo.clone(),
                    due_at: run.due_at,
                    box_id: *box_id,
                    at: at.clone(),
                    try_n: *try_n,
                })
            }
            _ => None,
        })
        .collect()
}

/// Why and where the breaker tripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDisable {
    /// The repo whose runs keep failing.
    pub repo: ForgeRepo,
    /// What to tell the owner.
    pub reason: String,
}

/// Whether the workflow should be disabled: some repo's latest [`BREAKER_STREAK`] concluded runs
/// were all refused, failed, incomplete or timed out.
///
/// Only runs due since the workflow was last enabled count, so re-enabling restarts the count. A
/// skipped run neither counts nor resets the streak, and a run still in flight is not concluded,
/// so it does neither either. A delivered run resets it, and a declined one does neither. Nothing
/// retries a failed run; the next firing is the only retry.
#[must_use]
pub fn workflow_breaker(workflow: &Workflow, runs: &[WorkflowRun]) -> Option<WorkflowDisable> {
    let WorkflowState::Enabled { since } = workflow.state else {
        return None;
    };
    workflow.repos.iter().find_map(|repo| {
        let mut mine: Vec<&WorkflowRun> = runs
            .iter()
            .filter(|r| &r.repo == repo && r.due_at >= since)
            .collect();
        mine.sort_by_key(|r| std::cmp::Reverse(r.due_at));
        let mut streak = 0;
        for run in mine {
            match &run.state {
                RunState::Done(RunEnd::Refused { .. }) => streak += 1,
                RunState::Done(RunEnd::Ended { outcome, .. }) => match outcome {
                    RunOutcome::Failed { .. }
                    | RunOutcome::Incomplete { .. }
                    | RunOutcome::TimedOut { .. } => streak += 1,
                    RunOutcome::Delivered { .. } => break,
                    RunOutcome::Declined { .. } => {}
                },
                RunState::Queued
                | RunState::Running { .. }
                | RunState::Waiting { .. }
                | RunState::Done(RunEnd::SkippedOverlap) => {}
            }
        }
        (streak >= BREAKER_STREAK).then(|| WorkflowDisable {
            repo: repo.clone(),
            reason: format!("{repo}: the last {BREAKER_STREAK} runs were refused or timed out"),
        })
    })
}

/// What a due firing does given the prior run: skip and record the skip while the prior run
/// blocks, so two unfinished runs never work the same repo of one workflow at once.
const fn overlap_policy(prior: Option<PriorRun>, due_at: Ts) -> WorkflowVerdict {
    match prior {
        Some(p) if p.blocks() => WorkflowVerdict::SkipOverlap { due_at },
        _ => WorkflowVerdict::Start { due_at },
    }
}

mod advance;
mod finished;
mod gate;
mod wait;
#[cfg(test)]
mod tests;

pub use advance::{
    advance, answer_move, quiet_move, quiet_since, refusal_move, report_move, Event, Move, CHECK_LOSSES,
};
pub use finished::{run_verdict, RunBoxFacts, RunVerdict, QUIET_BEFORE_FINISHED};
pub use gate::code_gate;
pub use wait::{waited_millis, workflow_unanswered, Unanswered, WAIT_LIMIT};
