//! Events about inbox rows that were never answered by the agent they were for.

use serde::Serialize;

/// An inbox row that will not be sent, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxEvent {
    /// A row sent to a launch that ended before its turn did; the new agent is told of it in a
    /// note that takes its place, and it is never sent again.
    SteerCut {
        /// The row, now the note.
        inbox: i64,
        /// Who sent it.
        sender: String,
    },
    /// A close deleted the inbox and these steers and prompts, never sent, with it.
    SteersDropped {
        /// The rows.
        inbox: Vec<i64>,
    },
}

impl InboxEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SteerCut { .. } => "steer_cut",
            Self::SteersDropped { .. } => "steers_dropped",
        }
    }
}
