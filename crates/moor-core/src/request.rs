//! Typed request model: one variant per command.

use serde::{Deserialize, Serialize};

use crate::name::{BoxName, SessionId};

mod binding;
pub use binding::{BindSessionReq, SessionBindingReq};
mod session;
pub use session::ShowSessionReq;
mod wait;
pub use wait::{WaitEventsReq, WaitFor, WaitMode, WaitReq};

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
    /// Wait for events from several Boxes.
    WaitEvents(WaitEventsReq),
    /// Send input to a task's agent.
    Steer(SteerReq),
    /// End a task's live agent session and keep its box.
    EndSession(EndSessionReq),
    /// Open a fresh agent session on a ready box, seeded with a first prompt.
    OpenSession(OpenSessionReq),
    /// Register a Session with no Box that runs on the host in a workdir.
    OpenHostSession(OpenHostSessionReq),
    /// Record the agent or stored agent session of a host Session.
    UpdateHostSession(UpdateHostSessionReq),
    /// End a host Session.
    EndHostSession(EndHostSessionReq),
    /// Bind a conversation key to a Session.
    BindSession(BindSessionReq),
    /// Look up the Session a conversation key is bound to.
    SessionBinding(SessionBindingReq),
    /// Show one Session with its child Sessions.
    ShowSession(ShowSessionReq),
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
    /// Wake a task's box and return the command that attaches to its workspace; the caller runs it.
    Connect(TaskReq),
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
            Self::WaitEvents(_) => "wait-events",
            Self::Steer(_) => "steer",
            Self::EndSession(_) => "end-session",
            Self::OpenSession(_) => "open-session",
            Self::OpenHostSession(_) => "open-host-session",
            Self::UpdateHostSession(_) => "update-host-session",
            Self::EndHostSession(_) => "end-host-session",
            Self::BindSession(_) => "bind-session",
            Self::SessionBinding(_) => "session-binding",
            Self::ShowSession(_) => "show-session",
            Self::Report(_) => "report",
            Self::Show(_) => "show",
            Self::Close(_) => "close",
            Self::Remove(_) => "remove",
            Self::Cp(_) => "cp",
            Self::Open(_) => "open",
            Self::Shell(_) => "shell",
            Self::Sleep(_) => "sleep",
            Self::Wake(_) => "wake",
            Self::Connect(_) => "connect",
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct StartReq {
    /// Box name.
    pub name: BoxName,
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
    /// Launch the agent with no first prompt.
    #[serde(default)]
    pub hold: bool,
    /// Resume the agent's stored session instead of starting a fresh one.
    #[serde(default)]
    pub resume: bool,
    /// Launch the agent with the Task's `.moor.yaml` even when it differs from its base commit.
    #[serde(default)]
    pub trust_moor_yaml: bool,
    /// Repo known to the controller, by name or url; resolved on the controller host instead of the invocation directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Parent Session of the Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}

/// Arguments of `run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct RunReq {
    /// Box name.
    pub task: BoxName,
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
    /// Run even when the Task's `.moor.yaml` differs from its base commit.
    #[serde(default)]
    pub trust_moor_yaml: bool,
    /// Program output goes to moor's own stdout and stderr instead of the run log.
    /// Never read from or written to JSON.
    #[serde(skip)]
    pub inherit_output: bool,
}

/// Arguments of `steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerReq {
    /// Box name.
    pub task: BoxName,
    /// Text to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Menu entry to pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<u32>,
}

/// Arguments of `end-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndSessionReq {
    /// Task name.
    pub task: String,
}

/// Arguments of `open-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionReq {
    /// Task name.
    pub task: String,
    /// Text the agent receives as its first prompt.
    pub seed: String,
    /// Parent Session of the Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}

/// Arguments of `open-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenHostSessionReq {
    /// Absolute directory the agent runs in on the host.
    pub workdir: String,
    /// Agent to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Stored agent session to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// Parent Session of the host Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}

/// Arguments of `update-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateHostSessionReq {
    /// Session id.
    pub session: SessionId,
    /// Agent to record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Stored agent session to record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

/// Arguments of `end-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndHostSessionReq {
    /// Session id.
    pub session: SessionId,
}

/// Arguments of `report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportReq {
    /// Box name.
    pub task: BoxName,
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
    /// Box name; all tasks when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<BoxName>,
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
    /// Box name.
    pub task: BoxName,
    /// Close despite unfinished work.
    #[serde(default)]
    pub force: bool,
    /// Run teardown with the worktree's `.moor.yaml` even when it differs from the base commit.
    #[serde(default)]
    pub trust_moor_yaml: bool,
}

/// Arguments of `remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveReq {
    /// Box name.
    pub task: BoxName,
    /// Discard unmerged work.
    #[serde(default)]
    pub discard: bool,
}

/// Arguments of `cp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpReq {
    /// Box name.
    pub task: BoxName,
    /// Source path.
    pub src: String,
    /// Destination path.
    pub dest: String,
}

/// Arguments of commands that take only a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskReq {
    /// Box name.
    pub task: BoxName,
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
mod host_tests;
#[cfg(test)]
mod tests;
