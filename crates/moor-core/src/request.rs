//! Typed request model: one variant per command.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;
use crate::name::{BoxName, SessionId};
use crate::user::Identity;

mod artifacts;
pub use artifacts::{ArtifactReadReq, ArtifactsReq};
mod binding;
pub use binding::{BindSessionReq, SessionBindingReq};
mod names;
pub use names::{Grant, Named, Pins};
mod session;
mod target;
pub use session::{EndSessionReq, OpenSessionReq, SessionsReq, ShowSessionReq};
pub use target::BoxTarget;
mod user;
pub use user::{
    ActAsReq, AsReq, LinkRedeemReq, ProxyReleaseReq, SecretDeleteReq, SecretSetReq, SecretValue,
    StartNeedsReq, UserCmd,
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
    /// Open a box's workspace.
    Open(BoxReq),
    /// Sleep a box.
    Sleep(BoxReq),
    /// Wake a box.
    Wake(BoxReq),
    /// Wake a box and return the command that attaches to its workspace; the caller runs it.
    Connect(BoxReq),
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
    /// Create a user for the caller's identity, or return the one it has.
    Enroll(AsReq),
    /// Store a secret for the caller's user, sealed with the vault key.
    SecretSet(SecretSetReq),
    /// Delete one of the caller's user's secrets.
    SecretDelete(SecretDeleteReq),
    /// List the names of the caller's user's secrets.
    SecretNames(AsReq),
    /// Mint a one-time code that links another identity to the caller's user.
    LinkCode(AsReq),
    /// Link the caller's identity to the user that minted a code.
    LinkRedeem(LinkRedeemReq),
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
            Self::Open(_) => "open",
            Self::Sleep(_) => "sleep",
            Self::Wake(_) => "wake",
            Self::Connect(_) => "connect",
            Self::Gate(_) => "gate",
            Self::Env(_) => "env",
            Self::Artifacts(_) => "artifacts",
            Self::ArtifactRead(_) => "artifact-read",
            Self::Whoami(_) => "whoami",
            Self::Enroll(_) => "enroll",
            Self::SecretSet(_) => "secret-set",
            Self::SecretDelete(_) => "secret-delete",
            Self::SecretNames(_) => "secret-names",
            Self::LinkCode(_) => "link-code",
            Self::LinkRedeem(_) => "link-redeem",
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
    /// Goal text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
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
    /// Launch the agent with no first prompt.
    #[serde(default)]
    pub hold: bool,
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
}

/// Arguments of `open-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenHostSessionReq {
    /// Absolute directory the agent runs in on the host. Only the operator chooses one; for
    /// anyone else the controller gives the Session a directory of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Agent to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Stored agent session to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    /// Parent Session of the host Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// The caller's own name for the Session, opaque to the controller. While the caller has
    /// a live host Session with this key, opening again returns that Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
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

/// Arguments of `cp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: BoxName,
    /// Source path.
    pub src: String,
    /// Destination path on the host, absolute.
    pub dest: String,
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
