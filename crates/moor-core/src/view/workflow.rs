//! Typed `value` payloads for the `workflow-*` requests.

use serde::{Deserialize, Serialize};

use crate::gate::GateDoc;

/// The front a workflow came from, as shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginView {
    /// The front's namespace.
    pub front: String,
    /// The key the front gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The latest run of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRunView {
    /// The scheduled time the run answers, in Unix milliseconds.
    pub due_at: i64,
    /// `starting`, `started`, `waiting`, `finished`, `failed`, `incomplete`, `declined`,
    /// `timed_out`, `skipped_overlap` or `refused`.
    pub outcome: String,
    /// `queued`, `running`, `waiting` or `done`.
    #[serde(default)]
    pub phase: String,
    /// The outcome moor found for a delivered run of a Box with gates, such as "check failed";
    /// `outcome` stays `finished`, since a bad outcome is still a run that did its job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<crate::gate::Verdict>,
    /// What fired the run: `cron`, `manual` or `pull_request`.
    #[serde(default = "cron")]
    pub trigger: String,
    /// The pull request the run is for, `OWNER/NAME#NUMBER`; none for a run for no pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
    /// The head commit of that pull request the run is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
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

/// The trigger of a run a writer that predates triggers described.
fn cron() -> String {
    "cron".to_owned()
}

/// One line of a run's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLineView {
    /// When it was written, in Unix milliseconds.
    pub at: i64,
    /// The line, tagged by its `kind`. A kind this build does not know shows as it was stored.
    pub entry: serde_json::Value,
}

/// The plan moor is making from a workflow's prompt, or why it made none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanMakingView {
    /// The hash of the prompt the plan is made from, as 64 hex digits.
    pub prompt: String,
    /// The Box making it, once asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_name: Option<String>,
    /// When the Box was asked for, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Why no plan was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// Who wrote a workflow's gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatesBy {
    /// A person; moor keeps them until asked to make its reading again.
    Person,
    /// moor, reading the prompt.
    Moor,
}

/// One workflow as `workflow-list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowView {
    /// Its name.
    pub name: String,
    /// Its cron expression; none when it has no cron.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// The zone the cron is read in; none when it has no cron.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The repos each run's Box clones, in order; the first is where the agent works.
    pub repos: Vec<String>,
    /// The partial repo config laid over the repo's for each of its Boxes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Its prompt.
    pub prompt: String,
    /// Whether it fires.
    pub enabled: bool,
    /// Minutes a run may take.
    pub timeout_minutes: u32,
    /// The next time it is due, in Unix milliseconds; none while disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due: Option<i64>,
    /// The plan each run walks, as `add_workflow` takes it; none when the workflow has none and
    /// each run delivers the prompt in one step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<serde_json::Value>,
    /// Whether moor's reading, its plan or gates, was made from a different prompt than the one
    /// the workflow has now. A stale reading still runs.
    #[serde(default)]
    pub plan_stale: bool,
    /// Who wrote the gates: `person` for gates moor keeps until asked to make its reading again,
    /// `moor` for its own reading of the prompt; none when it has no gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gates_by: Option<GatesBy>,
    /// The plan moor is making for the prompt, or why it made none; none when it is not making one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_making: Option<PlanMakingView>,
    /// Counts the writes of the workflow; send it as `expected_revision` to change it.
    #[serde(default)]
    pub revision: u64,
    /// Whether a pull request event on its first repo fires a run.
    #[serde(default)]
    pub on_pull_request: bool,
    /// The gates each run's Box starts with, as `add_workflow` takes them; none when the workflow
    /// has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gates: Option<Vec<GateDoc>>,
    /// The outcomes its runs may end in, as `add_workflow` takes them; none when the workflow
    /// names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<Vec<crate::gate::OutcomeDoc>>,
    /// Its latest run, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<LastRunView>,
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

/// Value of `workflow-answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowAnswerValue {
    /// The workflow's name.
    pub workflow: String,
    /// What the run did next, in one sentence.
    pub next: String,
}

/// Value of `workflow-run-now`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunNowValue {
    /// The workflow's name.
    pub workflow: String,
    /// `started` when a run began, `refused` when none did.
    pub status: String,
    /// The Box the run started in, when it started.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "box")]
    pub box_name: Option<String>,
    /// Why none began, when it was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
        /// The run's id, the same in each of its changes.
        #[serde(default)]
        id: i64,
        /// The title of the step the run is at, from its frozen plan; none when it is at no
        /// step.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_title: Option<String>,
        /// The front that created the workflow.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<OriginView>,
        /// The run.
        #[serde(flatten)]
        run: Box<LastRunView>,
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

/// Value of `step-report`: what the agent is told next, and whether it is an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepReportValue {
    /// What to do next, in words for the agent.
    pub reply: String,
    /// Whether the report was refused or has no run to count for, so the agent should act on
    /// the reply before it reports again.
    pub error: bool,
}

/// One run of a workflow, as `workflow-run-list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRowView {
    /// The run's id, to read it again alone.
    pub id: i64,
    /// The run.
    #[serde(flatten)]
    pub run: LastRunView,
    /// The plan frozen into the run, when one run was asked for; a run before plans reads as
    /// the single step of the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<serde_json::Value>,
}

/// Value of `workflow-run-read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunReadValue {
    /// The name of the run's workflow, as its owner knows it.
    pub workflow: String,
    /// Whether the reader owns the workflow, so may answer and fire it.
    pub yours: bool,
    /// The run with its frozen plan.
    pub run: RunRowView,
}

/// Value of `workflow-run-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunListValue {
    /// The workflow's name.
    pub workflow: String,
    /// The runs, newest first.
    pub runs: Vec<RunRowView>,
}
