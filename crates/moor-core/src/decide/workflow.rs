//! The workflow tick decider: whether a workflow has a firing due.

use crate::name::BoxId;
use crate::seam::clock::Ts;
use crate::workflow::{RunCause, RunId, RunState, StepId, Workflow, WorkflowRun, WorkflowState};

/// What the store and the Boxes say about a workflow's runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowFacts {
    /// The due time of the latest recorded cron firing, whatever came of it. A run fired by hand
    /// is left out, so it never swallows a due time the schedule missed.
    pub last_due: Option<Ts>,
    /// The latest run of any trigger that was a real attempt, looking through skipped and refused
    /// ones.
    pub prior: Option<PriorRun>,
}

/// A run that may keep the workflow's next firing from starting a Box.
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

/// What to do this tick.
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

/// Decides whether `workflow` has a firing due at `now`.
///
/// Only the latest due time at or before `now` fires, and only once: a workflow that missed
/// several times while the controller was down fires the newest and never a backlog. A due time
/// before the workflow was created or last enabled never fires.
#[must_use]
pub fn workflow_due(workflow: &Workflow, facts: &WorkflowFacts, now: Ts) -> WorkflowVerdict {
    let WorkflowState::Enabled { since } = workflow.state else {
        return WorkflowVerdict::NotDue;
    };
    let due = workflow
        .schedule
        .as_ref()
        .and_then(|s| s.latest_at_or_before(now))
        .filter(|&d| d >= since && facts.last_due.is_none_or(|l| d > l));
    due.map_or(WorkflowVerdict::NotDue, |due_at| {
        overlap_policy(facts.prior, due_at)
    })
}

/// A started run that has outlived its timeout, and the Box to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowExpiry {
    /// The run.
    pub run: RunId,
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
    /// The run.
    pub run: RunId,
    /// The due time the run answers.
    pub due_at: Ts,
    /// The Box the run recorded, none while it is still being started.
    pub box_id: Option<BoxId>,
    /// Whether the run only claimed its firing so far.
    pub claimed_only: bool,
    /// Why the run started, which its planned Box is named by.
    pub cause: RunCause,
}

/// When a Box's agent first started, if it did.
pub type AgentStarted<'a> = &'a dyn Fn(BoxId) -> Option<Ts>;

/// The milliseconds a run spent waiting for answers since the given time.
pub type Waited<'a> = &'a dyn Fn(RunId, Ts) -> i64;

/// The runs in `open` whose agent has not started [`START_LIMIT`] after the run was claimed.
///
/// The time the run spent waiting for answers since the claim is left out.
#[must_use]
pub fn workflow_start_overdue(
    open: &[WorkflowRun],
    (agent_started, waited): (AgentStarted<'_>, Waited<'_>),
    now: Ts,
) -> Vec<StartBreach> {
    let limit = i64::try_from(START_LIMIT.as_millis()).unwrap_or(i64::MAX);
    open.iter()
        .filter(|run| now.as_millis().saturating_sub(run.recorded_at.as_millis()) >= limit)
        .filter_map(|run| {
            let (box_id, claimed_only) = match run.state {
                RunState::Queued => (None, true),
                RunState::Running { box_id, .. }
                    if agent_started(box_id).is_none()
                        && now
                            .as_millis()
                            .saturating_sub(run.recorded_at.as_millis())
                            .saturating_sub(waited(run.id, run.recorded_at))
                            >= limit =>
                {
                    (Some(box_id), false)
                }
                _ => return None,
            };
            Some(StartBreach {
                run: run.id,
                due_at: run.due_at,
                box_id,
                claimed_only,
                cause: run.cause.clone(),
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
    agent_started: &dyn Fn(BoxId) -> Option<Ts>,
    waited: &dyn Fn(RunId, Ts) -> i64,
    now: Ts,
) -> Vec<WorkflowExpiry> {
    let limit = workflow.timeout.millis();
    open.iter()
        .filter_map(|run| match &run.state {
            RunState::Running { box_id, at, try_n }
                if agent_started(*box_id).is_some_and(|began| {
                    let spent = now.as_millis().saturating_sub(began.as_millis());
                    spent >= limit && spent.saturating_sub(waited(run.id, began)) >= limit
                }) =>
            {
                Some(WorkflowExpiry {
                    run: run.id,
                    box_id: *box_id,
                    at: at.clone(),
                    try_n: *try_n,
                })
            }
            _ => None,
        })
        .collect()
}

/// What a due firing does given the prior run: skip and record the skip while the prior run
/// blocks, so two unfinished runs of one workflow never work at once.
const fn overlap_policy(prior: Option<PriorRun>, due_at: Ts) -> WorkflowVerdict {
    match prior {
        Some(p) if p.blocks() => WorkflowVerdict::SkipOverlap { due_at },
        _ => WorkflowVerdict::Start { due_at },
    }
}

mod advance;
mod finished;
mod gate;
#[cfg(test)]
mod tests;
mod wait;

pub use advance::{
    advance, answer_move, quiet_move, quiet_since, refusal_move, report_move, Event, Move,
    CHECK_LOSSES,
};
pub use finished::{run_verdict, RunBoxFacts, RunVerdict, QUIET_BEFORE_FINISHED};
pub use gate::code_gate;
pub use wait::{waited_millis, workflow_unanswered, Unanswered, WAIT_LIMIT};
