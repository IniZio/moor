//! Typed `value` payloads for `start`, `show` and `close` (JSON wire shape).

pub mod env;
pub mod run;
pub mod sleep;
pub mod steer;

use crate::domain::boxes::BoxState;
use crate::domain::task::TaskState;

/// Why the agent could not be launched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchFailure {
    /// The launch step that failed.
    pub step: String,
    /// What went wrong.
    pub message: String,
    /// The last lines of output, when the failure printed any.
    pub tail: Option<String>,
    /// RFC3339 UTC time of the failure.
    pub at: String,
}

impl LaunchFailure {
    /// The text `start` shows for the failure.
    #[must_use]
    pub fn text(&self) -> String {
        format!(
            "agent dispatch failed: {}: {}; re-run start to retry",
            self.step, self.message
        )
    }
}

/// Agent activity on a Task row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentView {
    /// Whether the agent was launched.
    pub launched: bool,
    /// `live`, `exited` or `gone`.
    pub pane: String,
    /// Agent state name; `failed` when the launch failed.
    pub state: String,
    /// Where the state was learned.
    pub source: String,
    /// The agent's exit code, once it ended.
    pub exit_code: Option<i32>,
    /// What the agent waits for, if anything.
    pub waiting_for: Option<String>,
    /// Why the launch failed, when it did.
    pub reason: Option<LaunchFailure>,
}

/// The in-flight run on a Task row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunView {
    /// Run id.
    pub id: i64,
    /// Command argv.
    pub argv: Vec<String>,
    /// RFC3339 UTC start time.
    pub started_at: String,
}

/// Sleep configuration and the reason a Box will not sleep.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoxSleepView {
    /// Configured idle time before the Box sleeps, if any.
    pub after: Option<String>,
    /// Why the Box cannot sleep, when known.
    pub reason: Option<String>,
}

/// A Task's Box.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoxView {
    /// Box lifecycle state.
    pub state: BoxState,
    /// Provider id.
    pub provider: String,
    /// Box handle, when one is known.
    pub handle: Option<String>,
    /// Working directory inside the Box, when known.
    pub workdir: Option<String>,
    /// `none` (open egress), `enforced` (allow-list applied) or `not_enforced` (allow-list not applied); unset when unknown.
    pub egress: Option<String>,
    /// RFC3339 UTC time the Box went to sleep.
    pub asleep_since: Option<String>,
    /// Sleep configuration and blocking reason.
    pub sleep: BoxSleepView,
}

/// The shared Task row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TaskView {
    /// Task id.
    pub id: i64,
    /// Task name.
    pub name: String,
    /// Repo path or alias.
    pub repo: String,
    /// Issue reference.
    pub issue: Option<String>,
    /// Goal text.
    pub goal: Option<String>,
    /// Task state.
    pub state: TaskState,
    /// Why the Task is in this state.
    pub state_reason: Option<String>,
    /// Base ref.
    pub base: Option<String>,
    /// Branch name.
    pub branch: Option<String>,
    /// Worktree path.
    pub worktree: Option<String>,
    /// Workspace id.
    pub workspace: Option<String>,
    /// The Box, when one exists.
    pub r#box: Option<BoxView>,
    /// Agent activity.
    pub agent: Option<AgentView>,
    /// In-flight run.
    pub run: Option<RunView>,
    /// RFC3339 UTC last update.
    pub updated_at: String,
}

/// Outcome of the setup script.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct SetupView {
    /// Whether setup ran.
    pub ran: bool,
    /// Whether it succeeded.
    pub ok: bool,
    /// Last output line.
    pub line: Option<String>,
}

/// Outcome of dotfiles delivery.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct DotfilesView {
    /// Whether dotfiles ran.
    pub ran: bool,
    /// Whether it succeeded.
    pub ok: bool,
    /// Duration in milliseconds.
    pub ms: Option<u64>,
}

/// Outcome of the agent launch.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct AgentLaunchView {
    /// Whether an agent was launched.
    pub launched: bool,
    /// Agent session id.
    pub session: Option<String>,
    /// Why the launch failed, when it did.
    pub reason: Option<LaunchFailure>,
}

/// `value` of `start`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StartView {
    /// The Task row.
    pub task: TaskView,
    /// Task already existed.
    pub attached: bool,
    /// Task is queued for capacity.
    pub queued: bool,
    /// Names promoted from the queue.
    pub promoted: Vec<String>,
    /// Setup outcome.
    pub setup: SetupView,
    /// Dotfiles outcome.
    pub dotfiles: DotfilesView,
    /// Agent launch outcome.
    pub agent: AgentLaunchView,
    /// Warnings raised while starting.
    pub warnings_inline: Vec<String>,
}

/// One event row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EventView {
    /// Event id.
    pub id: i64,
    /// Snake-case kind.
    pub kind: String,
    /// Structured payload.
    pub payload: serde_json::Value,
    /// RFC3339 UTC time.
    pub ts: String,
}

/// Pane tail section of `show`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PaneView {
    /// Last lines of the pane.
    pub tail: String,
    /// Pane status.
    pub status: String,
}

/// One report row of `show`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReportView {
    /// Report id.
    pub id: i64,
    /// Report kind.
    pub kind: String,
    /// Report version.
    pub version: u32,
    /// Report title.
    pub title: Option<String>,
    /// Report path.
    pub path: String,
}

/// `value` of `show`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShowView {
    /// Per-Task rows.
    pub tasks: Vec<TaskView>,
    /// The selected Task.
    pub task: Option<TaskView>,
    /// Events, when requested.
    pub events: Option<Vec<EventView>>,
    /// Pane, when requested.
    pub pane: Option<PaneView>,
    /// Reports, when requested.
    pub reports: Option<Vec<ReportView>>,
}

/// `value` of `close`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CloseView {
    /// The Task row.
    pub task: TaskView,
    /// Box was deleted.
    pub box_deleted: bool,
    /// Workspace was closed.
    pub workspace_closed: bool,
    /// Agent was stopped.
    pub agent_stopped: bool,
    /// Transcript path.
    pub transcript: Option<String>,
    /// Worktree was kept.
    pub worktree_kept: bool,
    /// Names promoted from the queue.
    pub promoted: Vec<String>,
}

#[cfg(test)]
mod tests;
