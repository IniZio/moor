//! Events about the agent's conversation when new hands replace lost ones.

use serde::Serialize;

/// What new hands got back of the agent's conversation before it resumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreEvent {
    /// The conversation is back up to its last saved turn.
    SessionRestored {
        /// When the restored archive was taken, RFC 3339.
        archive_at: String,
    },
    /// The conversation could not be brought back, so the agent started without it.
    SessionLost {
        /// Why the conversation could not be restored.
        reason: String,
    },
}

impl RestoreEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionRestored { .. } => "session_restored",
            Self::SessionLost { .. } => "session_lost",
        }
    }
}
