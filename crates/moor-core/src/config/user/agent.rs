//! The `agent:` section of the user config: the Box agent and what holds its terminal.

use serde::Deserialize;

/// `agent:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentConfig {
    /// Default model.
    pub model: Option<String>,
    /// Whether moor installs the agent on a Box that lacks it; on by default.
    pub install: Option<bool>,
    /// What holds the agent's terminal on the Box; hc when unset.
    pub holder: Option<AgentHolder>,
}

impl AgentConfig {
    /// What holds the agent's terminal on the Box.
    #[must_use]
    pub fn holder(&self) -> AgentHolder {
        self.holder.unwrap_or_default()
    }
}

/// `agent.holder`: the Box program that keeps the agent's terminal alive across dropped links.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentHolder {
    /// herdr-core, which moor installs on a Box that lacks it.
    #[default]
    Hc,
    /// tmux, which the Box image brings.
    Tmux,
}
