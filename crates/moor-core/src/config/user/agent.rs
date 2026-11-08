//! The `agent:` section of the user config: the Box agent.

use serde::Deserialize;

/// `agent:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentConfig {
    /// Default model.
    pub model: Option<String>,
    /// Whether moor installs the agent on a Box that lacks it; on by default.
    pub install: Option<bool>,
}
