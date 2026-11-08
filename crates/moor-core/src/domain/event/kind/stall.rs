//! Events about an agent that stopped answering: a busy agent gone quiet and its return, or a
//! held prompt it could not be given.

use serde::Serialize;

/// The agent stalled, reported again after it had, or could not be given a held prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
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
    /// A prompt held for a start that queued could not be given to the agent.
    PromptNotGiven {
        /// Which try this was, from 1.
        attempt: u32,
        /// What went wrong.
        error: String,
    },
}

impl StallEvent {
    /// The kind tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AgentStalled { .. } => "agent_stalled",
            Self::AgentResumed { .. } => "agent_resumed",
            Self::PromptNotGiven { .. } => "prompt_not_given",
        }
    }
}
