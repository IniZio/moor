//! Typed `value` payloads for `start`, `show` and `close` (JSON wire shape).

pub mod ask;
pub mod attach;
pub mod attention;
pub mod binding;
pub mod box_config;
pub mod close_failed;
pub mod detail;
pub mod env;
pub mod forward;
pub mod gate;
pub mod inspect;
pub mod machine;
pub mod node;
pub mod output;
pub mod progress;
pub mod pulls;
pub mod repo_config;
pub mod run;
pub mod session;
pub mod setup;
pub mod sleep;
pub mod steer;
pub mod stuck;
pub mod task_box;
pub mod ties;
pub mod user;
pub mod workflow;
use crate::domain::{box_state::BoxLifecycle, hands::HandsState, session_state::SessionState};
pub use attach::{AttachEnd, AttachView, BoxRunState, BoxRunView, ExecView};
pub use attention::AttentionView;
pub use detail::{ApprovalView, SaidView};
pub use forward::{ForwardView, ForwardedView};
pub use inspect::{InspectView, StuckView};
pub use machine::{HeldMachineView, MachineHoldState};
pub use setup::{DotfilesView, SetupView};
pub use sleep::unfinished_turn;
pub use task_box::{TaskBoxState, TaskBoxView};
pub use ties::BoxTies;

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
    /// `permission` or `question` for a prompt the agent said the kind of.
    pub kind: Option<String>,
    /// What the prompt shows above its menu, such as the command a permission is for.
    pub detail: Option<String>,
    /// The background work an agent in `waiting` waits on, or that still runs beside the question
    /// an agent in `needs_input` asked in its reply; empty otherwise.
    pub waiting_on: Vec<String>,
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
    pub state: HandsState,
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

/// The shared Box row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoxView {
    /// Box id.
    pub id: i64,
    /// Box name.
    pub name: String,
    /// The repo the Box clones, as `OWNER/NAME`; `None` for a Box with no repo.
    pub repo: Option<crate::name::ForgeRepo>,
    /// The repos listed after the primary one, in listed order; omitted for a Box of one repo.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_repos: Vec<crate::name::ForgeRepo>,
    /// Issue reference.
    pub issue: Option<String>,
    /// Prompt text.
    pub prompt: Option<String>,
    /// Box lifecycle state.
    pub lifecycle: BoxLifecycle,
    /// Agent session state.
    pub session_state: SessionState,
    /// The Box is asleep with its agent mid-turn; waking it resumes the turn.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unfinished_turn: bool,
    /// Why the Box is in this state.
    pub state_reason: Option<String>,
    /// Base ref.
    pub base: Option<String>,
    /// Branch name.
    pub branch: Option<String>,
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
    /// The agent's newest message, on `show` of one Box asked for the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<SaidView>,
    /// The Box as a Box: its derived state and the line that matters now; set by `show`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::progress::ProgressView>,
    /// The approve step of a workflow run that waits in this Box, on `show`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalView>,
    /// The gates the Box must pass before it counts as done, and where it stands; set by `show`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<gate::GateView>,
    /// The workflow run and machine the Box is tied to; set by `show`.
    #[serde(skip_serializing_if = "BoxTies::is_empty")]
    pub ties: BoxTies,
    /// Who the Box acts for, by email, when that is not the viewer: set by `show` for everyone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// When the task was asked, RFC3339 UTC, for a place a question claimed: that task has no
    /// work Box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_at: Option<String>,
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
    /// The node the Box was placed on; absent when none is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
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
    /// The selected Box's events as a timeline, when events were requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Vec<crate::progress::TimelineEntry>>,
    /// The work Boxes a claimed place's agent started, oldest first; empty for any other Box.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub task_boxes: Vec<TaskBoxView>,
    /// What the selected Box's agent made outside moor, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<output::OutputView>,
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
    /// Agent was stopped.
    pub agent_stopped: bool,
    /// Transcript path.
    pub transcript: Option<String>,
}

#[cfg(test)]
mod tests;
