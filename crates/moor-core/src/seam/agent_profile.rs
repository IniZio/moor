//! Agent profile seam: turns an agent's hook state file into a neutral record.

use crate::activity::HookRecord;

/// What is specific to one kind of agent.
pub trait AgentProfile: Send + Sync {
    /// Parses the agent's hook state file; `None` when it is unreadable or partial.
    fn parse_hook(&self, raw: &[u8]) -> Option<HookRecord>;
}
