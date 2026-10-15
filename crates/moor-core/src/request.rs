//! Typed request model: one variant per command.

use serde::{Deserialize, Serialize};

use crate::name::TaskName;

/// One moor command with its arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// Create a task.
    Start(StartReq),
    /// Run a command in a task's box.
    Run(RunReq),
    /// Wait for a task condition.
    Wait(WaitReq),
    /// Send input to a task's agent.
    Steer(SteerReq),
    /// Publish a report.
    Report(ReportReq),
    /// Show tasks or one task.
    Show(ShowReq),
    /// Close a task.
    Close(CloseReq),
    /// Remove a task.
    Remove(RemoveReq),
    /// Copy a file into a task's box.
    Cp(CpReq),
    /// Open a task's workspace.
    Open(TaskReq),
    /// Open a shell in a task's box.
    Shell(TaskReq),
    /// Sleep a task's box.
    Sleep(TaskReq),
    /// Wake a task's box.
    Wake(TaskReq),
    /// Check a gate.
    Gate(GateReq),
    /// Show the layered environment.
    Env(EnvReq),
    /// Set up GitHub credentials.
    GithubSetup {},
    /// Log in to GitHub.
    GithubLogin {},
    /// Check the credential proxy.
    ProxyCheck {},
    /// Print version information.
    Version {},
}

impl Request {
    /// The command name; equals the serde tag.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Start(_) => "start",
            Self::Run(_) => "run",
            Self::Wait(_) => "wait",
            Self::Steer(_) => "steer",
            Self::Report(_) => "report",
            Self::Show(_) => "show",
            Self::Close(_) => "close",
            Self::Remove(_) => "remove",
            Self::Cp(_) => "cp",
            Self::Open(_) => "open",
            Self::Shell(_) => "shell",
            Self::Sleep(_) => "sleep",
            Self::Wake(_) => "wake",
            Self::Gate(_) => "gate",
            Self::Env(_) => "env",
            Self::GithubSetup {} => "github-setup",
            Self::GithubLogin {} => "github-login",
            Self::ProxyCheck {} => "proxy-check",
            Self::Version {} => "version",
        }
    }
}

/// Arguments of `start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartReq {
    /// Task name.
    pub name: TaskName,
    /// Goal text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// Issue reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// Worktree path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Base revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Agent model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Skip starting an agent.
    #[serde(default)]
    pub no_agent: bool,
    /// Resume the agent's stored session instead of starting a fresh one.
    #[serde(default)]
    pub resume: bool,
    /// Run setup and launch the agent even when the Task's `.moor.yaml` differs from its base commit.
    #[serde(default)]
    pub trust_moor_yaml: bool,
}

/// Arguments of `run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReq {
    /// Task name.
    pub task: TaskName,
    /// Skip syncing before the run.
    #[serde(default)]
    pub no_sync: bool,
    /// Sync before the run, but only when the Box holds no unsaved work.
    #[serde(default)]
    pub sync: bool,
    /// Record the run under this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// Command and arguments.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Program output goes to moor's own stdout and stderr instead of the run log.
    /// Never read from or written to JSON.
    #[serde(skip)]
    pub inherit_output: bool,
}

/// Arguments of `wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitReq {
    /// Task name.
    pub task: TaskName,
    /// Condition to wait for.
    #[serde(rename = "for", default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<WaitFor>,
    /// Timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Condition for `wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitFor {
    /// Agent idle.
    Idle,
    /// Task done.
    Done,
    /// Run finished.
    Run,
}

/// Arguments of `steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerReq {
    /// Task name.
    pub task: TaskName,
    /// Text to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Menu entry to pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<u32>,
}

/// Arguments of `report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportReq {
    /// Task name.
    pub task: TaskName,
    /// Report kind.
    pub kind: String,
    /// Report title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Report body file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// Arguments of `show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowReq {
    /// Task name; all tasks when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskName>,
    /// Number of events to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<u32>,
    /// Include agent state.
    #[serde(default)]
    pub agent: bool,
    /// Include reports.
    #[serde(default)]
    pub reports: bool,
    /// Include pane contents.
    #[serde(default)]
    pub pane: bool,
}

/// Arguments of `close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseReq {
    /// Task name.
    pub task: TaskName,
    /// Close despite unfinished work.
    #[serde(default)]
    pub force: bool,
}

/// Arguments of `remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveReq {
    /// Task name.
    pub task: TaskName,
    /// Discard unmerged work.
    #[serde(default)]
    pub discard: bool,
}

/// Arguments of `cp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpReq {
    /// Task name.
    pub task: TaskName,
    /// Source path.
    pub src: String,
    /// Destination path.
    pub dest: String,
}

/// Arguments of commands that take only a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskReq {
    /// Task name.
    pub task: TaskName,
}

/// Arguments of `gate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateReq {
    /// Commit sha.
    pub sha: String,
    /// Gate name.
    pub name: String,
    /// Command to run for the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// Arguments of `env`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvReq {
    /// Repository whose layer to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

#[cfg(test)]
mod tests;
