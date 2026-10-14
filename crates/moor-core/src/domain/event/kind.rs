//! Kind name, payload and assertion helpers for [`Event`].

use super::{Event, ASSERTED_KINDS};

impl Event {
    /// The `snake_case` kind name; equals the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Rejected { .. } => "rejected",
            Self::WaitingForBox { .. } => "waiting_for_box",
            Self::SettingUp { .. } => "setting_up",
            Self::Working => "working",
            Self::SetupFailed { .. } => "setup_failed",
            Self::SetupFinished { .. } => "setup_finished",
            Self::CredentialFailed { .. } => "credential_failed",
            Self::Idled => "idled",
            Self::Busy => "busy",
            Self::Asleep { .. } => "asleep",
            Self::Woke => "woke",
            Self::Closed { .. } => "closed",
            Self::BoxRequested => "box_requested",
            Self::BoxReady => "box_ready",
            Self::BoxFailed { .. } => "box_failed",
            Self::BoxAsleep => "box_asleep",
            Self::BoxGone => "box_gone",
            Self::BoxDeleted => "box_deleted",
            Self::BoxReleaseFailed { .. } => "box_release_failed",
            Self::StaysUp { .. } => "stays_up",
            Self::Ran { .. } => "ran",
            Self::Copied { .. } => "copied",
            Self::SessionStarted { .. } => "session_started",
            Self::SessionFailed { .. } => "session_failed",
            Self::Relaunched { .. } => "relaunched",
            Self::Checked { .. } => "checked",
            Self::RunStarted { .. } => "run_started",
            Self::Attached => "attached",
            Self::SessionsStopped { .. } => "sessions_stopped",
            Self::AgentPromptBlocked { .. } => "agent_prompt_blocked",
            Self::AgentPromptCheckFailed { .. } => "agent_prompt_check_failed",
            Self::TranscriptSaved { .. } => "transcript_saved",
            Self::TranscriptFailed { .. } => "transcript_failed",
            Self::Steered { .. } => "steered",
            Self::ProxyStarted { .. } => "proxy_started",
            Self::TunnelRestarted { .. } => "tunnel_restarted",
            Self::EgressRefused { .. } => "egress_refused",
            Self::ProxyCredentialFailed { .. } => "proxy_credential_failed",
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
