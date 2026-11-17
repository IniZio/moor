//! Shared nodes: where Boxes run.

use std::num::NonZeroU32;

use serde::Deserialize;

use super::{ConfigError, UserConfig};
use crate::name::NodeName;

/// `nodes.<name>:` section: a shared place that runs Boxes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NodeConfig {
    /// The crabbox provider the node's Boxes run on.
    pub runtime: String,
    /// Labels a Box may require of the node.
    pub labels: Vec<String>,
    /// Cap on live Boxes on the node (at least 1); unlimited when unset.
    pub max_boxes: Option<NonZeroU32>,
}

impl NodeConfig {
    /// The name and runtime are well-formed.
    pub(super) fn validate(&self, name: &str) -> Result<(), ConfigError> {
        NodeName::parse(name)
            .map_err(|e| ConfigError::new(format!("nodes.{name}"), e.to_string()))?;
        if self.runtime.trim().is_empty() {
            return Err(ConfigError::new(
                format!("nodes.{name}.runtime"),
                "runtime must name a crabbox provider",
            ));
        }
        if self.runtime.contains('@') {
            return Err(ConfigError::new(
                format!("nodes.{name}.runtime"),
                "runtime must be a bare provider id; a machine is a personal node, \
                 enrolled with `moor-machine-agent --runtime`",
            ));
        }
        Ok(())
    }
}

impl UserConfig {
    /// The shared node tried first: `box.node`, else the only configured node.
    #[must_use]
    pub fn default_node(&self) -> Option<&str> {
        match (&self.box_.node, self.nodes.len()) {
            (Some(n), _) => Some(n.as_str()),
            (None, 1) => self.nodes.keys().next().map(String::as_str),
            _ => None,
        }
    }

    /// Every node and `box.node` is well-formed. `box.node` may name an enrolled machine's node,
    /// which only the store knows.
    pub(super) fn validate_nodes(&self) -> Result<(), ConfigError> {
        for (name, n) in &self.nodes {
            n.validate(name)?;
        }
        if let Some(n) = &self.box_.node {
            NodeName::parse(n).map_err(|e| ConfigError::new("box.node", e.to_string()))?;
        }
        Ok(())
    }
}
