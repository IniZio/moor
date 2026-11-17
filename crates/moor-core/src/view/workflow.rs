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
    /// `starting`, `started`, `waiting`, `finished`, `failed`, `incomplete`, `declined`,
    /// `timed_out`, `skipped_overlap` or `refused`.
    pub outcome: String,
    /// `queued`, `running`, `waiting` or `done`.
    #[serde(default)]
    pub phase: String,
    /// The step a run in flight is at, or the step a failed, incomplete, declined or timed-out
    /// run ended at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// What a delivered run said it delivered, when it said anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Why it was refused or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The Box it started.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "box")]
    pub box_name: Option<String>,
    /// The name the run's Box has or will have, for a run that is starting or was refused. No
    /// Box of that name need exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_box: Option<String>,
    /// What happened to the run, oldest line first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryLineView>,
}

/// One line of a run's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLineView {
    /// When it was written, in Unix milliseconds.
    pub at: i64,
    /// The line, tagged by its `kind`. A kind this build does not know shows as it was stored.
    pub entry: serde_json::Value,
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
    /// The repos each of its Boxes clones beside its own.
    #[serde(default)]
    pub also: Vec<String>,
    /// The partial repo config laid over the repo's for each of its Boxes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
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
    /// The plan each run walks, as `add_workflow` takes it; none when the workflow has none and
    /// each run delivers the prompt in one step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<serde_json::Value>,
    /// Whether the plan was made from a different prompt than the one the workflow has now. A
    /// stale plan still runs.
    #[serde(default)]
    pub plan_stale: bool,
    /// Counts the writes of the workflow; send it as `expected_revision` to change it.
    #[serde(default)]
    pub revision: u64,
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
    /// After a `workflow-add`: the revision stored, to send as `expected_revision` next time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// After a `workflow-add`: the stored plan as a numbered list, for its author to review
    /// next to the prompt; empty when the workflow has no plan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<String>,
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
