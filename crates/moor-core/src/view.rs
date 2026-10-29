//! Typed `value` payloads for `start`, `show` and `close` (JSON wire shape).

pub mod attach;
pub mod binding;
pub mod env;
pub mod machine;
pub mod progress;
pub mod run;
pub mod session;
pub mod sleep;
pub mod steer;
pub mod user;
pub use attach::{AttachEnd, AttachView};
pub use machine::{HeldMachineView, MachineHoldState};

use crate::domain::box_state::BoxLifecycle;
use crate::domain::boxes::BoxState;
use crate::domain::session_state::SessionState;

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

/// Agent activity on a Box row.
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
    /// The prompt the agent waits on, if any.
    pub question: Option<String>,
    /// The numbered choices the prompt offers; empty when none.
    pub options: Vec<String>,
    /// Why the launch failed, when it did.
    pub reason: Option<LaunchFailure>,
}

/// The in-flight run on a Box row.
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

/// A Box.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DriverView {
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
    /// CPUs the Box delivers, as read inside it; unset until a start has read them.
    pub cpus: Option<u32>,
    /// Memory the Box delivers in MiB, as read inside it; unset until a start has read it.
    pub mem_mib: Option<u64>,
    /// RFC3339 UTC time the Box went to sleep.
    pub asleep_since: Option<String>,
    /// Sleep configuration and blocking reason.
    pub sleep: BoxSleepView,
}

/// A problem with the agent's model account that needs the user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AttentionView {
    /// Problem kind: `agent_rate_limited` or `proxy_credential_rejected`.
    pub kind: String,
    /// Credential rule the problem belongs to.
    pub rule: String,
    /// Upstream HTTP status.
    pub status: u16,
    /// When the limit resets, if known.
    pub resets_at: Option<String>,
    /// Human-readable summary.
    pub message: String,
}

/// The shared Box row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoxView {
    /// Box id.
    pub id: i64,
    /// Box name.
    pub name: String,
    /// The repo the Box clones, as `OWNER/NAME`; `None` for a Box with no repo.
    pub repo: Option<crate::name::ForgeRepo>,
    /// Issue reference.
    pub issue: Option<String>,
    /// Prompt text.
    pub prompt: Option<String>,
    /// Box lifecycle state.
    pub lifecycle: BoxLifecycle,
    /// Agent session state.
    pub session_state: SessionState,
    /// Why the Box is in this state.
    pub state_reason: Option<String>,
    /// Base ref.
    pub base: Option<String>,
    /// Branch name.
    pub branch: Option<String>,
    /// Workspace id.
    pub workspace: Option<String>,
    /// The Box, when one exists.
    pub driver: Option<DriverView>,
    /// Agent activity.
    pub agent: Option<AgentView>,
    /// In-flight run.
    pub run: Option<RunView>,
    /// RFC3339 UTC last update.
    pub updated_at: String,
    /// Open problems with the agent's model account; non-empty means the Box needs attention.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<AttentionView>,
    /// The user machine the Box holds, on `show` of one Box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_machine: Option<HeldMachineView>,
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
    /// Duration in milliseconds.
    pub ms: Option<u64>,
    /// Last lines of the setup log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
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
    /// The random token this launch's starting hook marker carries; never output.
    #[serde(skip)]
    pub nonce: Option<String>,
}

/// How a Box that keeps its repo work tree started in it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkTreeView {
    /// The kept tree was brought up to date, so the build cache it holds is warm.
    Refreshed,
    /// There was no tree to refresh: the Box cloned fresh.
    Cold,
    /// The refresh ran and failed, so the Box cloned fresh.
    ColdAfter {
        /// Why the refresh failed.
        reason: String,
    },
}

impl WorkTreeView {
    /// The state a refresh outcome names: `refreshed`, `cold` or `cold after: <reason>`.
    #[must_use]
    pub fn parse(outcome: &str) -> Option<Self> {
        Some(match outcome {
            "refreshed" => Self::Refreshed,
            "cold" => Self::Cold,
            other => Self::ColdAfter {
                reason: other.strip_prefix("cold after: ")?.to_owned(),
            },
        })
    }
}

/// `value` of `start`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StartView {
    /// The Box row.
    #[serde(rename = "box")]
    pub boxed: BoxView,
    /// Box already existed.
    pub attached: bool,
    /// Box is queued for capacity.
    pub queued: bool,
    /// Names promoted from the queue.
    pub promoted: Vec<String>,
    /// How the work tree started; absent when the Box keeps none or did not provision this call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_tree: Option<WorkTreeView>,
    /// Setup outcome.
    pub setup: SetupView,
    /// Dotfiles outcome.
    pub dotfiles: DotfilesView,
    /// Agent launch outcome.
    pub agent: AgentLaunchView,
    /// Warnings raised while starting.
    pub warnings_inline: Vec<String>,
    /// The provisioning step that failed while the Box carried on, such as `setup`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    /// Handle of the Box kept after a failed step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
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

/// One artifact row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactView {
    /// Artifact name.
    pub name: String,
    /// Artifact version.
    pub version: u32,
    /// Artifact kind.
    pub kind: String,
    /// MIME content type.
    pub content_type: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the content, hex.
    pub sha256: String,
    /// Creation time.
    pub created_at: String,
}

/// `value` of `artifact-read`, sent after the bytes frames of one byte range.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactReadView {
    /// Byte offset the range starts at.
    pub offset: u64,
    /// Bytes sent in frames.
    pub sent: u64,
    /// Size of the whole file in bytes.
    pub size: u64,
    /// SHA-256 of the whole file, hex.
    pub sha256: String,
    /// The range reaches the end of the file.
    pub eof: bool,
}

/// `value` of `show`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShowView {
    /// Per-Box rows; absent when a single Box is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boxes: Option<Vec<BoxView>>,
    /// The selected Box.
    #[serde(rename = "box")]
    pub boxed: Option<BoxView>,
    /// Events, when requested.
    pub events: Option<Vec<EventView>>,
    /// Pane, when requested.
    pub pane: Option<PaneView>,
    /// Reports, when requested.
    pub reports: Option<Vec<ReportView>>,
    /// Latest version of each artifact, when requested.
    pub artifacts: Option<Vec<ArtifactView>>,
}

/// `value` of `close`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CloseView {
    /// The Box row.
    #[serde(rename = "box")]
    pub boxed: BoxView,
    /// Box was deleted.
    pub box_deleted: bool,
    /// Workspace was closed.
    pub workspace_closed: bool,
    /// Agent was stopped.
    pub agent_stopped: bool,
    /// Transcript path.
    pub transcript: Option<String>,
    /// Names promoted from the queue.
    pub promoted: Vec<String>,
}

#[cfg(test)]
mod tests;
