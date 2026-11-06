//! Typed `value` payloads for the `workflow-*` requests.

use serde::{Deserialize, Serialize};

/// The front a workflow came from, as shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginView {
    /// The front's namespace.
    pub front: String,
    /// The key the front gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The latest run of one repo of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRunView {
    /// The repo.
    pub repo: String,
    /// The scheduled time the run answers, in Unix milliseconds.
    pub due_at: i64,
    /// `starting`, `started`, `finished`, `timed_out`, `skipped_overlap` or `refused`.
    pub outcome: String,
    /// Why it was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The Box it started.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "box")]
    pub box_name: Option<String>,
    /// The name the run's Box has or will have, for a run that is starting or was refused. No
    /// Box of that name need exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_box: Option<String>,
}

/// One workflow as `workflow-list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowView {
    /// Its name.
    pub name: String,
    /// Its cron expression.
    pub cron: String,
    /// The zone the cron is read in.
    pub timezone: String,
    /// Its repos.
    pub repos: Vec<String>,
    /// Its prompt.
    pub prompt: String,
    /// Whether it fires.
    pub enabled: bool,
    /// Why the controller disabled it, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// Minutes a run may take.
    pub timeout_minutes: u32,
    /// The next time it is due, in Unix milliseconds; none while disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due: Option<i64>,
    /// The latest run of each repo that has one.
    pub last_runs: Vec<LastRunView>,
}

/// Value of `workflow-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowListValue {
    /// The workflows by name.
    pub workflows: Vec<WorkflowView>,
}

/// Value of `workflow-add`, `workflow-remove` and `workflow-set-enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowChangeValue {
    /// The workflow's name.
    pub name: String,
    /// Whether anything changed.
    pub changed: bool,
    /// The next time it is due, in Unix milliseconds, when it is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due: Option<i64>,
}

/// One entry of the change stream `workflow-runs` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeView {
    /// A run was recorded or moved to its next outcome.
    Run {
        /// The position in the stream.
        seq: i64,
        /// The workflow's name.
        workflow: String,
        /// The front that created the workflow.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<OriginView>,
        /// The run.
        #[serde(flatten)]
        run: LastRunView,
    },
    /// The controller disabled the workflow.
    Disabled {
        /// The position in the stream.
        seq: i64,
        /// The workflow's name.
        workflow: String,
        /// The front that created the workflow.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<OriginView>,
        /// Why.
        reason: String,
    },
}

/// Value of `workflow-runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunsValue {
    /// The changes after the cursor, oldest first.
    pub changes: Vec<ChangeView>,
    /// The cursor to send next time; the one sent when there were no changes.
    pub next: i64,
}
