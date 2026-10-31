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
        Ok(())
    }
}

impl UserConfig {
    /// The node Boxes run on: `box.node`, else the only configured node.
    #[must_use]
    pub fn default_node(&self) -> Option<&str> {
        match (&self.box_.node, self.nodes.len()) {
            (Some(n), _) => Some(n.as_str()),
            (None, 1) => self.nodes.keys().next().map(String::as_str),
            _ => None,
        }
    }

    /// Every node is well-formed and `box.node` names one.
    pub(super) fn validate_nodes(&self) -> Result<(), ConfigError> {
        for (name, n) in &self.nodes {
            n.validate(name)?;
        }
        match &self.box_.node {
            Some(n) if !self.nodes.contains_key(n) => {
                let known: Vec<&str> = self.nodes.keys().map(String::as_str).collect();
                Err(ConfigError::new(
                    "box.node",
                    format!("no node named {n}; configured nodes: {}", known.join(", ")),
                ))
            }
            None if self.nodes.len() > 1 => Err(ConfigError::new(
                "box.node",
                "is required when nodes has more than one entry",
            )),
            _ => Ok(()),
        }
    }
}
