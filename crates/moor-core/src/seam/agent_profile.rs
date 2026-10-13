//! Agent profile seam: turns an agent's hook state file and pane text into neutral readings.

use crate::activity::HookRecord;

/// What is specific to one kind of agent.
pub trait AgentProfile: Send + Sync {
    /// Parses the agent's hook state file; `None` when it is unreadable or partial.
    fn parse_hook(&self, raw: &[u8]) -> Option<HookRecord>;

    /// The pending question in the pane text, apart from the agent's input box and footer.
    fn pane_prompt(&self, tail: &str) -> Option<String>;
}
