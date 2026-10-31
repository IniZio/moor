//! The `drivers.<name>:` section.

use std::num::NonZeroU32;

use serde::Deserialize;

use crate::config::{check, ConfigError};

/// `drivers.<name>:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DriverConfig {
    /// Removed key, kept only so a stale config gets a pointed error; `None` in any config
    /// that validates.
    #[serde(default, deserialize_with = "crate::config::controller::present_key")]
    #[allow(
        clippy::option_option,
        reason = "outer Some means the key was present, even when null"
    )]
    pub max_boxes: Option<Option<serde_json::Value>>,
    /// Seconds allowed to look up a Box's ssh target (at least 1); 5 by default,
    /// 20 for the `external` provider.
    pub lookup_timeout_secs: Option<NonZeroU32>,
    /// Environment variable names the provider's platform puts in the Box.
    /// moor removes them from every Box exec: setup, teardown, `moor run` and the agent.
    pub strip_env: Vec<String>,
    /// Whether this provider's Boxes get cache volumes; on by default.
    pub cache_volumes: Option<bool>,
}

impl DriverConfig {
    /// Whether this provider's Boxes get cache volumes.
    #[must_use]
    pub fn cache_volumes(&self) -> bool {
        self.cache_volumes.unwrap_or(true)
    }

    /// No removed key is set, and every `strip_env` entry is a valid variable name.
    pub(super) fn validate(&self, provider: &str) -> Result<(), ConfigError> {
        if self.max_boxes.is_some() {
            return Err(ConfigError::new(
                format!("drivers.{provider}.max_boxes"),
                format!(
                    "was removed; limit a node instead: `nodes: {{name: {{runtime: {provider}, max_boxes: N}}}}`"
                ),
            ));
        }
        for (i, name) in self.strip_env.iter().enumerate() {
            let here = format!("drivers.{provider}.strip_env[{i}]");
            check::env_name(name).map_err(|why| ConfigError::new(here, why))?;
        }
        Ok(())
    }
}
