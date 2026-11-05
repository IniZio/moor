//! Workflows: a stored, recurring definition of a Box start, and the record of each firing.

use std::fmt;

use crate::name::{BoxId, ForgeRepo};
use crate::seam::clock::Ts;
use crate::user::UserId;

/// A workflow definition that failed validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowError {
    /// The name is not 1-64 characters of lowercase letters, digits, `.`, `_` or `-`.
    #[error("invalid workflow name `{0}`: use 1-64 lowercase letters, digits, '.', '_' or '-', starting with a letter or digit")]
    Name(String),
    /// The schedule is not a five-field cron expression.
    #[error("invalid cron `{expr}`: {reason}")]
    Cron {
        /// The text that was rejected.
        expr: String,
        /// Why it was rejected.
        reason: String,
    },
    /// The time zone is not in the IANA database.
    #[error("unknown time zone `{0}`: use an IANA name such as Europe/London")]
    Zone(String),
    /// A workflow needs at least one repo.
    #[error("a workflow needs at least one repo")]
    NoRepos,
    /// The same repo is listed twice.
    #[error("repo `{0}` is listed twice")]
    DuplicateRepo(String),
    /// The prompt is empty.
    #[error("a workflow needs a prompt")]
    NoPrompt,
    /// The timeout is outside 1-720 minutes.
    #[error("invalid timeout of {0} minutes: use 1 to 720")]
    Timeout(u32),
    /// Two firings of the cron can be closer than the floor.
    #[error("cron `{expr}` can fire twice within {floor_minutes} minutes; fire less often")]
    TooFrequent {
        /// The expression that was rejected.
        expr: String,
        /// The shortest gap allowed, in whole minutes.
        floor_minutes: u64,
    },
}

/// How many failed runs in a row, for one repo of one workflow, disable the workflow.
pub const BREAKER_STREAK: usize = 3;

/// The database id of a workflow.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct WorkflowId(i64);

impl WorkflowId {
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

impl fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A workflow's name, unique per owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkflowName(String);

impl WorkflowName {
    /// Validates a name: 1-64 bytes of `[a-z0-9._-]`, the first a letter or digit.
    ///
    /// # Errors
    /// [`WorkflowError::Name`] for any other text.
    pub fn parse(s: &str) -> Result<Self, WorkflowError> {
        let mut bytes = s.bytes();
        let ok = (1..=64).contains(&s.len())
            && bytes
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes.all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            });
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(WorkflowError::Name(s.to_owned()))
        }
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkflowName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether a workflow fires, and since when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowState {
    /// Fires for due times from `since` on, the moment it was created or last enabled.
    Enabled {
        /// When it was created or last enabled.
        since: Ts,
    },
    /// Never fires.
    Disabled {
        /// Why the controller disabled it, when its owner did not.
        reason: Option<String>,
    },
}

/// A workflow as its owner wrote it, before the store gives it an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDef {
    /// Unique per owner.
    pub name: WorkflowName,
    /// When it fires.
    pub schedule: WorkflowSchedule,
    /// The repos it starts a Box in, each firing; never empty, never repeated.
    pub repos: Vec<ForgeRepo>,
    /// The opaque prompt each started Box gets.
    pub prompt: String,
    /// Whether it should fire.
    pub enabled: bool,
    /// How long a started run may take.
    pub timeout: TimeoutMinutes,
}

impl WorkflowDef {
    /// Builds a definition, checking the repos and the prompt.
    ///
    /// # Errors
    /// [`WorkflowError::NoRepos`], [`WorkflowError::DuplicateRepo`] or
    /// [`WorkflowError::NoPrompt`].
    pub fn new(
        name: WorkflowName,
        schedule: WorkflowSchedule,
        repos: Vec<ForgeRepo>,
        prompt: &str,
        (enabled, timeout): (bool, TimeoutMinutes),
    ) -> Result<Self, WorkflowError> {
        if repos.is_empty() {
            return Err(WorkflowError::NoRepos);
        }
        for (i, repo) in repos.iter().enumerate() {
            if repos.iter().take(i).any(|seen| seen == repo) {
                return Err(WorkflowError::DuplicateRepo(repo.to_string()));
            }
        }
        if prompt.trim().is_empty() {
            return Err(WorkflowError::NoPrompt);
        }
        Ok(Self {
            name,
            schedule,
            repos,
            prompt: prompt.to_owned(),
            enabled,
            timeout,
        })
    }
}

/// A stored workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    /// The database id.
    pub id: WorkflowId,
    /// Whose Boxes it starts.
    pub owner: UserId,
    /// Unique per owner.
    pub name: WorkflowName,
    /// When it fires.
    pub schedule: WorkflowSchedule,
    /// The repos it starts a Box in, each firing.
    pub repos: Vec<ForgeRepo>,
    /// The opaque prompt each started Box gets.
    pub prompt: String,
    /// Whether it fires.
    pub state: WorkflowState,
    /// How long a started run may take.
    pub timeout: TimeoutMinutes,
    /// When it was created.
    pub created_at: Ts,
    /// When it was last changed.
    pub updated_at: Ts,
}

/// What came of one due time for one repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutcome {
    /// A Box was started and has not ended.
    Started {
        /// The Box.
        box_id: BoxId,
    },
    /// The Box finished before its timeout.
    Finished {
        /// The Box.
        box_id: BoxId,
        /// When the run ended.
        ended_at: Ts,
    },
    /// The Box was still running at its timeout and was expired.
    TimedOut {
        /// The Box.
        box_id: BoxId,
        /// When the run was expired.
        ended_at: Ts,
    },
    /// The previous firing's Box was still open, so none was started.
    SkippedOverlap,
    /// The start was refused.
    Refused {
        /// Why, as the start reported it.
        detail: String,
    },
}

/// How a started run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// It finished before its timeout.
    Finished,
    /// It ran to its timeout.
    TimedOut,
}

/// What a firing records: one repo at one due time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    /// The workflow.
    pub workflow: WorkflowId,
    /// The repo.
    pub repo: ForgeRepo,
    /// The scheduled time this firing answers.
    pub due_at: Ts,
    /// What came of it.
    pub outcome: WorkflowOutcome,
    /// When it was recorded, which for a started run is when it started.
    pub recorded_at: Ts,
}

mod schedule;
#[cfg(test)]
mod tests;

pub use schedule::{TimeoutMinutes, WorkflowSchedule, MIN_FIRING_INTERVAL};
