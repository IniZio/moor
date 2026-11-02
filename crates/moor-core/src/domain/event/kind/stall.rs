//! Events about a busy agent that stopped reporting, and its return.

use serde::Serialize;

/// The agent stalled, or reported again after it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StallEvent {
    /// The agent's hook reported busy and then went quiet for too long.
    AgentStalled {
        /// What the agent was doing at its last report.
        doing: Option<String>,
        /// Whole seconds since that report.
        quiet_secs: u64,
    },
    /// The agent reported again after a stall.
    AgentResumed {
        /// Whole seconds the agent had been quiet.
        quiet_secs: u64,
    },
}

impl StallEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AgentStalled { .. } => "agent_stalled",
            Self::AgentResumed { .. } => "agent_resumed",
        }
    }
}
