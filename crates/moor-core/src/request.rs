//! Typed request model: one variant per command.

use serde::{Deserialize, Serialize};

use crate::name::{BoxName, ForgeRepo, SessionId};
use crate::user::Identity;

mod artifacts;
pub use artifacts::{ArtifactReadReq, ArtifactsReq};
mod attach;
pub use attach::AttachReq;
mod basic;
pub use basic::{BoxReq, CloseFailedReq, CloseReq, GateReq};
mod binding;
pub use binding::{BindSessionReq, SessionBindingReq};
mod env;
pub use env::EnvReq;
mod exec;
pub use exec::ExecReq;
mod files;
pub use files::{BoxRsyncReq, CpReq};
mod inspect;
pub use inspect::InspectReq;
mod host_session;
pub use host_session::{EndHostSessionReq, OpenHostSessionReq, UpdateHostSessionReq};
mod machine;
pub use machine::{
    BoxHostMemory, BoxStage, HoldOp, HoldValue, MachineBox, MachineBoxOp, MachineBoxReq,
    MachineEnrollReq, MachineHoldReq, MachineLink, MachineLinksReq, MachineRemoveReq,
    MachineStatusReq, MemoryReport, RunnerAddr, WarmStarts,
};
mod names;
pub use names::{Grant, Named, Pins};
mod session;
mod target;
pub use session::{EndSessionReq, OpenSessionReq, SessionsReq, ShowSessionReq};
pub use target::BoxTarget;
mod cmd;
mod user;
mod workflow;
pub use user::{
    ActAsReq, AsReq, EnrollReq, McpAddReq, McpOAuthFinishReq, McpRemoveReq, ProfileSetReq,
    ProxyReleaseReq, SecretDeleteReq, SecretSetReq, SecretValue, StartNeedsReq, UserCmd,
    WebhookAddReq, WebhookRemoveReq, WebhookSetupFinishReq, WebhookSetupShowReq,
};
pub use workflow::{
    WorkflowAddReq, WorkflowCmd, WorkflowEnableReq, WorkflowRemoveReq, WorkflowRunsReq,
};
mod wait;
pub use wait::{WaitEventsReq, WaitFor, WaitMode, WaitReq};
mod webhook_outbox;
pub use webhook_outbox::{WebhookClaimReq, WebhookOutcome, WebhookReportReq};

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
    /// Look at a stuck-looking Box: its pane, recent messages and why it is stuck.
    Inspect(InspectReq),
    /// Close a box.
    Close(CloseReq),
    /// Copy a file into a box.
    Cp(CpReq),
    /// Run the machine runner's rsync in the Box with its stdio streamed both ways over this
    /// connection. It takes over the connection: the controller socket closes it after the answer.
    BoxRsync(BoxRsyncReq),
    /// Open a box's workspace.
    Open(BoxReq),
    /// Sleep a box.
    Sleep(BoxReq),
    /// Wake a box.
    Wake(BoxReq),
    /// Re-run setup on a Box whose setup failed.
    Retry(BoxReq),
    /// Close every Box whose setup failed that the caller may act on.
    CloseFailed(CloseFailedReq),
    /// Wake a box and return the command that attaches to its workspace; the caller runs it.
    Connect(BoxReq),
    /// Relay the agent's terminal on the Box both ways over this connection until either side
    /// ends it. It takes over the connection: the controller socket closes it after the answer.
    Attach(AttachReq),
    /// Run a command in the Box with its stdio streamed both ways over this connection until it
    /// ends. It takes over the connection: the controller socket closes it after the answer.
    Exec(ExecReq),
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
    /// Add a webhook for the caller's user, waiting for its person to set it up.
    WebhookAdd(WebhookAddReq),
    /// One of the caller's user's webhooks for its setup page, with its key while waiting.
    WebhookSetupShow(WebhookSetupShowReq),
    /// Store a webhook's header value, if it has a header, and start its deliveries.
    WebhookSetupFinish(WebhookSetupFinishReq),
    /// List the caller's user's webhooks; never their keys.
    WebhookList(AsReq),
    /// Remove one of the caller's user's webhooks.
    WebhookRemove(WebhookRemoveReq),
    /// Add or replace one of the caller's user's workflows.
    WorkflowAdd(WorkflowAddReq),
    /// List the caller's user's workflows with their next due time and last runs.
    WorkflowList(AsReq),
    /// Remove one of the caller's user's workflows and its runs.
    WorkflowRemove(WorkflowRemoveReq),
    /// Enable or disable one of the caller's user's workflows.
    WorkflowSetEnabled(WorkflowEnableReq),
    /// The changes to the caller's user's workflow runs after a cursor, oldest first.
    WorkflowRuns(WorkflowRunsReq),
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
    /// List the nodes the caller may start Boxes on; the operator sees every node.
    NodeList(AsReq),
    /// Ask the controller whether each node runtime's provider credentials are in place.
    ProviderReady(AsReq),
    /// Ask one of the caller's user's machine agents for its status.
    MachineStatus(MachineStatusReq),
    /// List every enrolled machine's key; the operator's only.
    MachineKeys {},
    /// Record which enrolled machines hold a link to the runner; the operator's only.
    MachineLinks(MachineLinksReq),
    /// Read or write the runner's record of a Box on a machine; the runner's and operator's only.
    MachineBox(MachineBoxReq),
    /// Claim one of a Box's user's machines for it, or let go of it; the operator's only.
    #[serde(rename = "machine-hold")]
    MachineHold(MachineHoldReq),
    /// Lease the front Proxy for the caller's user until the lease is released or expires.
    ProxyLease(AsReq),
    /// End a lease `proxy-lease` granted.
    ProxyRelease(ProxyReleaseReq),
    /// Whether the front Proxy is running.
    ProxyStatus(AsReq),
    /// Lease the oldest pending webhook deliveries; only a front with `webhooks: true`.
    WebhookClaim(WebhookClaimReq),
    /// End a claimed webhook delivery with its outcome; only a front with `webhooks: true`.
    WebhookReport(WebhookReportReq),
    /// Run another request as an identity a front asserts.
    As(ActAsReq),
    /// Print version information.
    Version {},
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
    /// One of the requester's nodes to run the Box on, in place of their default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Skip starting an agent.
    #[serde(default)]
    pub no_agent: bool,
    /// Set the Box up from scratch, past any setup kept from earlier Boxes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cold: bool,
    /// Resume the agent's stored session instead of starting a fresh one.
    #[serde(default)]
    pub resume: bool,
    /// The repo the Box clones; a Box with no repo starts with an empty workdir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// Repos the Box clones beside `repo`, in order; `box.also` in the user config when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub also: Option<Vec<ForgeRepo>>,
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

#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod tests;
