//! Kind name, payload and assertion helpers for [`Event`].

use super::Event;

mod node_disk;
mod stall;
pub use node_disk::NodeDiskEvent;
pub use stall::StallEvent;

/// The event kinds the event tests pin. Frozen.
pub const ASSERTED_KINDS: [&str; 20] = [
    "started",
    "rejected",
    "pending",
    "setting_up",
    "working",
    "setup_failed",
    "idled",
    "busy",
    "asleep",
    "woke",
    "closed",
    "box_ready",
    "box_failed",
    "box_gone",
    "box_deleted",
    "box_asleep",
    "session_started",
    "ran",
    "copied",
    "credential_failed",
];

/// Every event kind name an [`Event`] can carry.
pub const EVENT_KINDS: [&str; 81] = [
    "caches_not_saved",
    "disk_lost",
    "started",
    "rejected",
    "pending",
    "setting_up",
    "working",
    "setup_failed",
    "credential_failed",
    "idled",
    "busy",
    "asleep",
    "woke",
    "closed",
    "box_requested",
    "box_progress",
    "box_ready",
    "box_failed",
    "box_asleep",
    "box_gone",
    "box_deleted",
    "box_release_failed",
    "stays_up",
    "ran",
    "copied",
    "session_started",
    "session_ended",
    "session_failed",
    "launch_timed_out",
    "relaunch_capped",
    "relaunched",
    "checked",
    "run_started",
    "attached",
    "sessions_stopped",
    "agent_prompt_blocked",
    "agent_said",
    "agent_doing",
    "needs_input",
    "agent_prompt_check_failed",
    "transcript_saved",
    "transcript_failed",
    "transcript_skipped",
    "steered",
    "proxy_started",
    "tunnel_restarted",
    "egress_refused",
    "egress_aborted",
    "credential_withheld",
    "proxy_credential_failed",
    "agent_rate_limited",
    "proxy_credential_rejected",
    "proxy_credential_recovered",
    "agent_link_lost",
    "agent_link_restored",
    "agent_unreachable",
    "agent_stalled",
    "agent_resumed",
    "node_disk_low",
    "node_disk_recovered",
    "turn_done",
    "artifact_published",
    "machine_attached",
    "machine_left_out",
    "machine_forward_failed",
    "machine_detached",
    "machine_waiting",
    "machine_refused",
    "profile_resolved",
    "profile_failed",
    "setup_finished",
    "cloned",
    "step_started",
    "step_finished",
    "setup_line_started",
    "setup_line_finished",
    "warm_started",
    "warm_start_failed",
    "cache_cold",
    "checkpoint_taken",
    "checkpoint_evicted",
];

impl Event {
    /// The `snake_case` kind name; equals the serde tag.
    #[must_use]
    #[allow(clippy::too_many_lines, reason = "one arm per event variant")]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Rejected { .. } => "rejected",
            Self::Pending { .. } => "pending",
            Self::SettingUp { .. } => "setting_up",
            Self::Working => "working",
            Self::SetupFailed { .. } => "setup_failed",
            Self::CredentialFailed { .. } => "credential_failed",
            Self::Idled => "idled",
            Self::Busy => "busy",
            Self::Asleep { .. } => "asleep",
            Self::Woke => "woke",
            Self::Closed { .. } => "closed",
            Self::BoxRequested => "box_requested",
            Self::BoxProgress { .. } => "box_progress",
            Self::BoxReady => "box_ready",
            Self::BoxFailed { .. } => "box_failed",
            Self::BoxAsleep => "box_asleep",
            Self::BoxGone => "box_gone",
            Self::BoxDeleted => "box_deleted",
            Self::BoxReleaseFailed { .. } => "box_release_failed",
            Self::StaysUp { .. } => "stays_up",
            Self::DiskLost => "disk_lost",
            Self::CachesNotSaved => "caches_not_saved",
            Self::Ran { .. } => "ran",
            Self::Copied { .. } => "copied",
            Self::SessionStarted { .. } => "session_started",
            Self::SessionEnded { .. } => "session_ended",
            Self::SessionFailed { .. } => "session_failed",
            Self::LaunchTimedOut { .. } => "launch_timed_out",
            Self::RelaunchCapped { .. } => "relaunch_capped",
            Self::Relaunched { .. } => "relaunched",
            Self::Checked { .. } => "checked",
            Self::RunStarted { .. } => "run_started",
            Self::Attached => "attached",
            Self::SessionsStopped { .. } => "sessions_stopped",
            Self::AgentPromptBlocked { .. } => "agent_prompt_blocked",
            Self::AgentSaid { .. } => "agent_said",
            Self::AgentDoing { .. } => "agent_doing",
            Self::NeedsInput { .. } => "needs_input",
            Self::AgentPromptCheckFailed { .. } => "agent_prompt_check_failed",
            Self::TranscriptSaved { .. } => "transcript_saved",
            Self::TranscriptFailed { .. } => "transcript_failed",
            Self::TranscriptSkipped { .. } => "transcript_skipped",
            Self::Steered { .. } => "steered",
            Self::ProxyStarted { .. } => "proxy_started",
            Self::TunnelRestarted { .. } => "tunnel_restarted",
            Self::EgressRefused { .. } => "egress_refused",
            Self::EgressAborted { .. } => "egress_aborted",
            Self::CredentialWithheld { .. } => "credential_withheld",
            Self::ProxyCredentialFailed { .. } => "proxy_credential_failed",
            Self::AgentRateLimited { .. } => "agent_rate_limited",
            Self::ProxyCredentialRejected { .. } => "proxy_credential_rejected",
            Self::ProxyCredentialRecovered { .. } => "proxy_credential_recovered",
            Self::AgentLinkLost { .. } => "agent_link_lost",
            Self::AgentLinkRestored { .. } => "agent_link_restored",
            Self::AgentUnreachable { .. } => "agent_unreachable",
            Self::TurnDone { .. } => "turn_done",
            Self::ArtifactPublished { .. } => "artifact_published",
            Self::Machine(m) => m.kind(),
            Self::Profile(p) => p.kind(),
            Self::Stall(s) => s.kind(),
            Self::NodeDisk(d) => d.kind(),
            Self::Timing(t) => t.kind(),
        }
    }

    /// The event fields as a JSON object without the `kind` tag; `{}` for unit variants.
    #[must_use]
    pub fn payload(&self) -> serde_json::Value {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(mut map)) => {
                map.remove("kind");
                serde_json::Value::Object(map)
            }
            _ => serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Whether the event tests pin this kind.
    #[must_use]
    pub fn is_asserted(&self) -> bool {
        ASSERTED_KINDS.contains(&self.kind())
    }
}
