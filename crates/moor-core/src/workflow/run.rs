//! A run: one due time for one repo, from its claim to its end.

use std::fmt;

use super::{CommitSha, Plan, RunCause, StepId, WorkflowId};
use crate::domain::event::close_reason::CloseReason;
use crate::name::{BoxId, ForgeRepo};
use crate::seam::clock::Ts;

/// The database id of a run (`workflow_runs.id`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RunId(i64);

impl RunId {
    /// Wraps a raw id.
    #[must_use]
    pub const fn new(v: i64) -> Self {
        Self(v)
    }

    /// Returns the raw id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What fired a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTrigger {
    /// The workflow's schedule came due.
    Cron,
    /// Its owner asked for one run now.
    Manual,
    /// A pull request event.
    PullRequest,
}

impl RunTrigger {
    /// The name the store and `list_workflow_runs` give it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cron => "cron",
            Self::Manual => "manual",
            Self::PullRequest => "pull_request",
        }
    }

    /// The trigger a stored name stands for.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Cron, Self::Manual, Self::PullRequest]
            .into_iter()
            .find(|t| t.as_str() == name)
    }
}

/// Where one due time for one repo stands.
///
/// The phase and the outcome are kept apart, so a run in flight has no outcome and a run that
/// never got a Box has no step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// The firing is claimed and its Box is being started. It is recorded before the start, so a
    /// crash leaves it behind and the firing is never repeated.
    Queued,
    /// The Box is up and the run is at step `at`, try `try_n`.
    Running {
        /// The Box.
        box_id: BoxId,
        /// The step the run is at.
        at: StepId,
        /// How often the run has entered `at`, from 1.
        try_n: u8,
    },
    /// An approve step waits for the owner. The Box may sleep.
    Waiting {
        /// The Box.
        box_id: BoxId,
        /// The step the run waits at.
        at: StepId,
        /// How often the run has entered `at`, from 1.
        try_n: u8,
        /// When the wait began.
        since: Ts,
    },
    /// Nothing more will happen.
    Done(RunEnd),
}

/// How a run that is done ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// The previous firing's Box was still open, so none was started.
    SkippedOverlap,
    /// A pull request from a fork whose author lacks write access to the base repo, so none was
    /// started.
    SkippedFork {
        /// The author's GitHub login.
        author: String,
    },
    /// The start was refused, or no agent could run in the Box.
    Refused {
        /// Why, as the start reported it.
        detail: String,
    },
    /// A newer head or the pull request's close cancelled the run while its Box was being
    /// started. A Box the start still makes is closed when the start settles.
    CancelledUnstarted {
        /// What ended it.
        cause: CancelCause,
    },
    /// The run had a Box and ended.
    Ended {
        /// The Box.
        box_id: BoxId,
        /// When the run ended.
        ended_at: Ts,
        /// How.
        outcome: RunOutcome,
    },
}

/// How a run that had a Box ended. Only moor's own facts. What a delivery means is the agent's
/// text in `result`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// A deliver step reported ok, or the agent went quiet at the last step.
    Delivered {
        /// What the agent said it delivered; empty when it said nothing.
        result: String,
        /// The outcome moor found for a run of a Box with gates; none for any other run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verdict: Option<crate::gate::Verdict>,
    },
    /// A step failed with no `fail` edge, or its fail target was exhausted too.
    Failed {
        /// The step.
        at: StepId,
        /// Why.
        reason: String,
    },
    /// The run stopped short at step `at`, for the reason `why` names.
    Incomplete {
        /// The step.
        at: StepId,
        /// What stopped it.
        #[serde(default, skip_serializing_if = "Unfinished::is_unreported")]
        why: Unfinished,
    },
    /// The owner said no at an approve step with no `fail` edge.
    Declined {
        /// The step.
        at: StepId,
    },
    /// The run worked past its timeout.
    TimedOut {
        /// The step the run was at.
        at: StepId,
    },
    /// A newer head or the pull request's close ended the run.
    Cancelled {
        /// The step the run was at.
        at: StepId,
        /// What ended it.
        cause: CancelCause,
    },
}

/// What stopped a run short of its end.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unfinished {
    /// The agent stopped without reporting its step, even after a reminder.
    #[default]
    Unreported,
    /// The run's Box was closed, or is gone, before the run ended, and nothing says who closed it.
    BoxClosed,
    /// The run's Box was closed for this reason before the run ended.
    ClosedBy(CloseReason),
}

impl Unfinished {
    /// Whether the agent stopped without reporting.
    #[must_use]
    pub const fn is_unreported(&self) -> bool {
        matches!(self, Self::Unreported)
    }

    /// Why a run that stopped at `at` is incomplete, in a sentence.
    #[must_use]
    pub fn says(&self, at: &StepId) -> String {
        match self {
            Self::Unreported => {
                "the agent stopped without reporting the step, even after a reminder".to_owned()
            }
            Self::BoxClosed => format!("its Box was closed while the run was at {at}"),
            Self::ClosedBy(r) => {
                format!("its Box was closed {} while the run was at {at}", r.says())
            }
        }
    }

    /// Why a run whose Box closed for `reason` stopped; none says only that it closed.
    #[must_use]
    pub fn box_closed(reason: Option<CloseReason>) -> Self {
        reason.map_or(Self::BoxClosed, Self::ClosedBy)
    }
}

/// What cancelled a pull request run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CancelCause {
    /// A push moved the pull request to head `sha`.
    NewerHead {
        /// The new head.
        sha: CommitSha,
    },
    /// The pull request was closed.
    Closed,
}

/// The phase as shown in lists, the way GitHub Actions shows a run's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    /// The Box is being started.
    Queued,
    /// The run works.
    Running,
    /// The run waits for its owner.
    Waiting,
    /// Nothing more will happen.
    Done,
}

impl RunPhase {
    /// The phase as shown.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Done => "done",
        }
    }
}

impl RunState {
    /// The phase this state is in.
    #[must_use]
    pub const fn phase(&self) -> RunPhase {
        match self {
            Self::Queued => RunPhase::Queued,
            Self::Running { .. } => RunPhase::Running,
            Self::Waiting { .. } => RunPhase::Waiting,
            Self::Done(_) => RunPhase::Done,
        }
    }

    /// The name the store and `list_workflow_runs` give this state's outcome column: a phase
    /// that is not done keeps its progress name, and a done run its ending.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Queued => "starting",
            Self::Running { .. } => "started",
            Self::Waiting { .. } => "waiting",
            Self::Done(RunEnd::SkippedOverlap) => "skipped_overlap",
            Self::Done(RunEnd::SkippedFork { .. }) => "skipped_fork",
            Self::Done(RunEnd::Refused { .. }) => "refused",
            Self::Done(RunEnd::CancelledUnstarted { .. }) => "cancelled",
            Self::Done(RunEnd::Ended { outcome, .. }) => match outcome {
                RunOutcome::Delivered { .. } => "finished",
                RunOutcome::Failed { .. } => "failed",
                RunOutcome::Incomplete { .. } => "incomplete",
                RunOutcome::Declined { .. } => "declined",
                RunOutcome::TimedOut { .. } => "timed_out",
                RunOutcome::Cancelled { .. } => "cancelled",
            },
        }
    }

    /// The step a run in flight is at, or the step a run that failed, stopped, was declined or
    /// timed out ended at. A delivered run and a run that never got a Box have none.
    #[must_use]
    pub const fn step(&self) -> Option<&StepId> {
        match self {
            Self::Running { at, .. } | Self::Waiting { at, .. } => Some(at),
            Self::Done(RunEnd::Ended { outcome, .. }) => match outcome {
                RunOutcome::Failed { at, .. }
                | RunOutcome::Incomplete { at, .. }
                | RunOutcome::Declined { at }
                | RunOutcome::TimedOut { at }
                | RunOutcome::Cancelled { at, .. } => Some(at),
                RunOutcome::Delivered { .. } => None,
            },
            Self::Queued
            | Self::Done(
                RunEnd::SkippedOverlap
                | RunEnd::SkippedFork { .. }
                | RunEnd::Refused { .. }
                | RunEnd::CancelledUnstarted { .. },
            ) => None,
        }
    }

    /// The Box this run started, if it started one.
    #[must_use]
    pub const fn box_id(&self) -> Option<BoxId> {
        match self {
            Self::Running { box_id, .. }
            | Self::Waiting { box_id, .. }
            | Self::Done(RunEnd::Ended { box_id, .. }) => Some(*box_id),
            Self::Queued
            | Self::Done(
                RunEnd::SkippedOverlap
                | RunEnd::SkippedFork { .. }
                | RunEnd::Refused { .. }
                | RunEnd::CancelledUnstarted { .. },
            ) => None,
        }
    }
}

/// A firing before the store gives it an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    /// The workflow.
    pub workflow: WorkflowId,
    /// The repo.
    pub repo: ForgeRepo,
    /// The scheduled time this firing answers.
    pub due_at: Ts,
    /// Where it stands.
    pub state: RunState,
    /// When it was recorded.
    pub recorded_at: Ts,
    /// The workflow's plan, frozen with the claim; none for a run that never starts a Box and
    /// for a workflow nobody planned.
    pub plan: Option<Plan>,
    /// Why it started, and the pull request it is for when it has one.
    pub cause: RunCause,
}

/// What a firing records: one repo at one due time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    /// The database id.
    pub id: RunId,
    /// The workflow.
    pub workflow: WorkflowId,
    /// The repo.
    pub repo: ForgeRepo,
    /// The scheduled time this firing answers.
    pub due_at: Ts,
    /// Where it stands.
    pub state: RunState,
    /// When it was recorded, which for a started run is when it started.
    pub recorded_at: Ts,
    /// Why it started, and the pull request it is for when it has one.
    pub cause: RunCause,
}
