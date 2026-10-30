//! Typed request model: one variant per command.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;
use crate::name::{BoxName, SessionId};
use crate::user::Identity;

mod artifacts;
pub use artifacts::{ArtifactReadReq, ArtifactsReq};
mod attach;
pub use attach::AttachReq;
mod binding;
pub use binding::{BindSessionReq, SessionBindingReq};
mod files;
pub use files::{BoxMirrorReq, BoxPutReq, CpReq};
mod host_session;
pub use host_session::{EndHostSessionReq, OpenHostSessionReq, UpdateHostSessionReq};
mod machine;
pub use machine::{
    HoldOp, HoldValue, MachineEnrollReq, MachineHoldReq, MachineLink, MachineLinksReq,
    MachineRemoveReq, MachineStatusReq,
};
mod names;
pub use names::{Grant, Named, Pins};
mod session;
mod target;
pub use session::{EndSessionReq, OpenSessionReq, SessionsReq, ShowSessionReq};
pub use target::BoxTarget;
mod user;
pub use user::{
    ActAsReq, AsReq, EnrollReq, McpAddReq, McpOAuthFinishReq, McpRemoveReq, ProfileSetReq,
    ProxyReleaseReq, SecretDeleteReq, SecretSetReq, SecretValue, StartNeedsReq, UserCmd,
};
mod wait;
pub use wait::{WaitEventsReq, WaitFor, WaitMode, WaitReq};

/// One moor command with its arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// Create a box.
    Start(StartReq),
    /// The secrets a start would need from the caller's user, and which it lacks.
    StartNeeds(StartNeedsReq),
    /// Run a command in a box.
    Run(RunReq),
    /// Wait for a box condition.
    Wait(WaitReq),
    /// Wait for events from several Boxes.
    WaitEvents(WaitEventsReq),
    /// Send input to a box's agent.
    Steer(SteerReq),
    /// End a Box's live agent session and keep the Box.
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
    /// List live Sessions, optionally only the children of one.
    Sessions(SessionsReq),
    /// Show boxes or one box.
    Show(ShowReq),
    /// Close a box.
    Close(CloseReq),
    /// Copy a file into a box.
    Cp(CpReq),
    /// Mirror a Box's work tree into a host directory.
    BoxMirror(BoxMirrorReq),
    /// Write one host file into a Box's work tree.
    BoxPut(BoxPutReq),
    /// Open a box's workspace.
    Open(BoxReq),
    /// Sleep a box.
    Sleep(BoxReq),
    /// Wake a box.
    Wake(BoxReq),
    /// Wake a box and return the command that attaches to its workspace; the caller runs it.
    Connect(BoxReq),
    /// Relay the agent's terminal on the Box both ways over this connection until either side
    /// ends it. It takes over the connection: the controller socket closes it after the answer.
    Attach(AttachReq),
    /// Check a gate.
    Gate(GateReq),
    /// Show the layered environment.
    Env(EnvReq),
    /// List a box's artifacts.
    Artifacts(ArtifactsReq),
    /// Read a byte range of one artifact.
    ArtifactRead(ArtifactReadReq),
    /// Show the caller's user and its linked identities.
    Whoami(AsReq),
    /// Create a user for the caller's identity, or return the one it has; a front adds the
    /// verified email that decides which user that is.
    Enroll(EnrollReq),
    /// Store a secret for the caller's user, sealed with the vault key.
    SecretSet(SecretSetReq),
    /// Delete one of the caller's user's secrets.
    SecretDelete(SecretDeleteReq),
    /// List the names of the caller's user's secrets.
    SecretNames(AsReq),
    /// Add or replace one of the caller's user's MCP servers, storing its key in the vault.
    McpAdd(McpAddReq),
    /// Remove one of the caller's user's MCP servers and its key.
    McpRemove(McpRemoveReq),
    /// List the caller's user's MCP servers; never their keys.
    McpList(AsReq),
    /// Record the caller's user's agent profile repo, replacing any earlier one.
    ProfileSet(ProfileSetReq),
    /// Show the caller's user's agent profile repo.
    ProfileShow(AsReq),
    /// Clear the caller's user's agent profile repo.
    ProfileRemove(AsReq),
    /// Finish signing the caller's user in to one of their MCP servers.
    #[serde(rename = "mcp-oauth-finish")]
    McpOAuthFinish(McpOAuthFinishReq),
    /// Remove one of the caller's user's machines.
    MachineRemove(MachineRemoveReq),
    /// Add or replace a machine its agent enrolls, creating the caller's user when needed; a
    /// front adds the verified email that decides which user that is.
    MachineEnroll(MachineEnrollReq),
    /// List the caller's user's machines.
    MachineList(AsReq),
    /// Ask one of the caller's user's machine agents for its status.
    MachineStatus(MachineStatusReq),
    /// List every enrolled machine's key; the operator's only.
    MachineKeys {},
    /// Record which enrolled machines hold a link to the runner; the operator's only.
    MachineLinks(MachineLinksReq),
    /// Claim one of a Box's user's machines for it, or let go of it; the operator's only.
    #[serde(rename = "machine-hold")]
    MachineHold(MachineHoldReq),
    /// Lease the front Proxy for the caller's user until the lease is released or expires.
    ProxyLease(AsReq),
    /// End a lease `proxy-lease` granted.
    ProxyRelease(ProxyReleaseReq),
    /// Whether the front Proxy is running.
    ProxyStatus(AsReq),
    /// Run another request as an identity a front asserts.
    As(ActAsReq),
    /// Print version information.
    Version {},
}

impl Request {
    /// The command name; equals the serde tag.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Start(_) => "start",
            Self::StartNeeds(_) => "start-needs",
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
            Self::Sessions(_) => "sessions",
            Self::Show(_) => "show",
            Self::Close(_) => "close",
            Self::Cp(_) => "cp",
            Self::BoxMirror(_) => "box-mirror",
            Self::BoxPut(_) => "box-put",
            Self::Open(_) => "open",
            Self::Sleep(_) => "sleep",
            Self::Wake(_) => "wake",
            Self::Connect(_) => "connect",
            Self::Attach(_) => "attach",
            Self::Gate(_) => "gate",
            Self::Env(_) => "env",
            Self::Artifacts(_) => "artifacts",
            Self::ArtifactRead(_) => "artifact-read",
            Self::Whoami(_) => "whoami",
            Self::Enroll(_) => "enroll",
            Self::SecretSet(_) => "secret-set",
            Self::SecretDelete(_) => "secret-delete",
            Self::SecretNames(_) => "secret-names",
            Self::McpAdd(_) => "mcp-add",
            Self::McpRemove(_) => "mcp-remove",
            Self::McpList(_) => "mcp-list",
            Self::ProfileSet(_) => "profile-set",
            Self::ProfileShow(_) => "profile-show",
            Self::ProfileRemove(_) => "profile-remove",
            Self::McpOAuthFinish(_) => "mcp-oauth-finish",
            Self::MachineRemove(_) => "machine-remove",
            Self::MachineEnroll(_) => "machine-enroll",
            Self::MachineList(_) => "machine-list",
            Self::MachineStatus(_) => "machine-status",
            Self::MachineKeys {} => "machine-keys",
            Self::MachineLinks(_) => "machine-links",
            Self::MachineHold(_) => "machine-hold",
            Self::ProxyLease(_) => "proxy-lease",
            Self::ProxyRelease(_) => "proxy-release",
            Self::ProxyStatus(_) => "proxy-status",
            Self::As(_) => "as",
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
    /// Prompt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Issue reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
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
    /// The repo the Box clones; a Box with no repo starts with an empty workdir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// Parent Session of the Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// The identity the Box acts for; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
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
    #[serde(rename = "box")]
    pub box_name: BoxName,
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

/// Arguments of `steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Text to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Menu entry to pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<u32>,
    /// Send the agent's interrupt key instead of text or a menu entry.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt: bool,
}

/// Arguments of `show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct ShowReq {
    /// The Box, in any state: by name, the newest Box of that name; all open Boxes when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "box")]
    pub target: Option<BoxTarget>,
    /// Number of events to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<u32>,
    /// Include agent state.
    #[serde(default)]
    pub agent: bool,
    /// Include reports.
    #[serde(default)]
    pub reports: bool,
    /// Include the latest version of each artifact.
    #[serde(default)]
    pub artifacts: bool,
    /// Include pane contents.
    #[serde(default)]
    pub pane: bool,
}

/// Arguments of `close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseReq {
    /// The Box: by name, the newest open Box of that name, else the newest closed one.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Close despite unfinished work.
    #[serde(default)]
    pub force: bool,
}

/// Arguments of commands that take only a box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: BoxName,
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
    /// Repo whose layer to show, read through the forge like a start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// Commit or ref to read the repo at; the default branch when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod tests;
