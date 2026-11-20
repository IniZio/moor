//! Workflows: a stored, recurring definition of a Box start, and the record of each firing.

use std::fmt;

use crate::name::ForgeRepo;
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
    /// Two repos one Box would clone share a name, and so a folder.
    #[error("repos `{0}` and `{1}` are both named {name}: each repo of a Box needs its own name", name = .1.name)]
    SameName(ForgeRepo, ForgeRepo),
    /// The prompt is empty.
    #[error("a workflow needs a prompt")]
    NoPrompt,
    /// A step id is not 1-32 bytes of lowercase letters, digits and `-`, starting with a letter.
    #[error(
        "invalid step id `{0}`: use 1-32 lowercase letters, digits or '-', starting with a letter"
    )]
    StepId(String),
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

/// How many failed runs in a row of one workflow disable it.
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

/// The front that created a workflow and the key it finds the workflow's runs by. Core never
/// reads either one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowOrigin {
    /// The namespace of the front socket that sent the request.
    pub front: String,
    /// The front's own key, if it sent one.
    pub key: Option<String>,
}

/// A workflow as its owner wrote it, before the store gives it an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDef {
    /// Unique per owner.
    pub name: WorkflowName,
    /// When it fires.
    pub schedule: WorkflowSchedule,
    /// The repos each firing's one Box clones, in order: the first is where its agent works, and
    /// the rest sit beside it. Never empty, and no two share a name, so a folder.
    pub repos: Vec<ForgeRepo>,
    /// A partial repo config laid over the repo's for each Box it starts; `None` for none.
    pub config: Option<String>,
    /// The opaque prompt each started Box gets.
    pub prompt: String,
    /// Whether it should fire.
    pub enabled: bool,
    /// How long a started run may take.
    pub timeout: TimeoutMinutes,
    /// The front that created it, if one did.
    pub origin: Option<WorkflowOrigin>,
    /// The steps a run walks; none stores no plan, which reads back as [`Plan::single`].
    pub plan: Option<Plan>,
}

impl WorkflowDef {
    /// Builds a definition, checking the repos and the prompt. The Box clones every repo into a
    /// folder of its name, so no two repos may share a name.
    ///
    /// # Errors
    /// [`WorkflowError::NoRepos`], [`WorkflowError::DuplicateRepo`],
    /// [`WorkflowError::SameName`] or [`WorkflowError::NoPrompt`].
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
        for (i, r) in repos.iter().enumerate() {
            let mut earlier = repos.iter().take(i);
            if let Some(prev) = earlier.find(|s| s.name.eq_ignore_ascii_case(&r.name)) {
                return Err(if prev == r {
                    WorkflowError::DuplicateRepo(r.to_string())
                } else {
                    WorkflowError::SameName(prev.clone(), r.clone())
                });
            }
        }
        if prompt.trim().is_empty() {
            return Err(WorkflowError::NoPrompt);
        }
        Ok(Self {
            name,
            schedule,
            repos,
            config: None,
            prompt: prompt.to_owned(),
            enabled,
            timeout,
            origin: None,
            plan: None,
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
    /// The repos each firing's one Box clones, in order; the first is where its agent works.
    pub repos: Vec<ForgeRepo>,
    /// A partial repo config laid over the repo's for each Box it starts.
    pub config: Option<String>,
    /// The opaque prompt each started Box gets.
    pub prompt: String,
    /// Whether it fires.
    pub state: WorkflowState,
    /// How long a started run may take.
    pub timeout: TimeoutMinutes,
    /// The front that created it, if one did.
    pub origin: Option<WorkflowOrigin>,
    /// When it was created.
    pub created_at: Ts,
    /// When it was last changed.
    pub updated_at: Ts,
    /// The steps a run walks. [`Plan::single`] of the prompt until a person writes one.
    pub plan: Plan,
    /// Counts the writes of the workflow, from 0.
    pub revision: u64,
}

mod box_name;
mod cause;
mod history;
mod plan;
#[cfg(test)]
mod plan_tests;
mod report;
mod run;
#[cfg(test)]
mod run_tests;
mod schedule;
#[cfg(test)]
mod tests;

pub use box_name::box_name;
pub use cause::{CommitSha, PullError, PullRef, PullTarget, RunCause};
pub use history::HistoryEntry;
pub use plan::{
    draft_prompt, is_draft_prompt, plan_first_prompt, split_plan_first, split_start_prompt,
    CheckSays, Evidence, EvidenceName, Next, Plan, PlanDoc, PlanError, PromptHash, StartStep, Step,
    StepDoc, StepId, StepKind, Tries, APPROVE_PLAN, PLAN_FILE, PLAN_QUESTION, REVISE_PLAN,
};
pub use report::{
    Answer, StepReport, StepResult, EVIDENCE_MAX, EVIDENCE_NAMES_MAX, EVIDENCE_NAME_MAX, NOTE_MAX,
};
pub use run::{
    CancelCause, NewRun, RunEnd, RunId, RunOutcome, RunPhase, RunState, RunTrigger, WorkflowRun,
};
pub use schedule::{TimeoutMinutes, WorkflowSchedule, MIN_FIRING_INTERVAL};

/// One entry of a user's change stream, ordered by `seq` across runs and workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowChange {
    /// The position in the stream, rising with every change.
    pub seq: i64,
    /// The workflow's name.
    pub workflow: WorkflowName,
    /// The front that created the workflow, if one did.
    pub origin: Option<WorkflowOrigin>,
    /// What changed.
    pub what: ChangeKind,
}

/// What a [`WorkflowChange`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    /// A run was recorded or moved to its next outcome.
    Run(Box<WorkflowRun>),
    /// The controller disabled the workflow for this reason.
    Disabled {
        /// Why.
        reason: String,
    },
}
