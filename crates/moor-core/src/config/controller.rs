//! The controller process config file: strict parse plus validation, no IO.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

use super::{parse_strict, ConfigError};
use crate::user::valid_front_namespace;

/// The `moor controller serve` config. Build it with [`parse_controller`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ControllerConfig {
    /// Unix socket path; `~` allowed. None means `<state dir>/controller.sock`.
    pub socket: Option<String>,
    /// moor state directory override; `~` allowed. None means the process default.
    pub state_dir: Option<String>,
    /// Seconds between passes that settle every open Box's agent state; must be above 0.
    /// None means 5.
    pub settle_every_secs: Option<u64>,
    /// Seconds a SIGTERM waits for in-flight requests before the controller exits anyway.
    /// None means 600.
    pub drain_timeout_secs: Option<u64>,
    /// Removed key, kept only so a stale config gets a pointed error.
    /// Double option so a present-but-null `web:` still counts as present.
    #[serde(default, deserialize_with = "present_key")]
    #[allow(
        clippy::option_option,
        reason = "outer Some means the key was present, even when null"
    )]
    web: Option<Option<serde_json::Value>>,
    /// Removed key, kept only so a stale config gets a pointed error.
    #[serde(default, deserialize_with = "present_key")]
    #[allow(
        clippy::option_option,
        reason = "outer Some means the key was present, even when null"
    )]
    slack: Option<Option<serde_json::Value>>,
    /// Fronts that each get a socket bound to their namespace, keyed by that namespace.
    pub fronts: BTreeMap<String, FrontEntry>,
}

/// One front's entry under `fronts:`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontEntry {
    /// The argv the controller runs and keeps running as this front; none leaves the front to
    /// be started by hand.
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

/// Wraps whatever follows the key in `Some`, so a null value stays distinguishable from a missing key.
#[allow(
    clippy::option_option,
    reason = "outer Some means the key was present, even when null"
)]
pub(super) fn present_key<'de, D>(d: D) -> Result<Option<Option<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<serde_json::Value>::deserialize(d).map(Some)
}

/// Seconds between settle passes when the config does not say.
const DEFAULT_SETTLE_SECS: u64 = 5;

/// Seconds a drain waits when the config does not say: long enough for a `start` to finish.
const DEFAULT_DRAIN_SECS: u64 = 600;

impl ControllerConfig {
    /// The time between settle passes.
    #[must_use]
    pub fn settle_every(&self) -> Duration {
        Duration::from_secs(self.settle_every_secs.unwrap_or(DEFAULT_SETTLE_SECS))
    }

    /// How long a SIGTERM waits for in-flight requests.
    #[must_use]
    pub fn drain_timeout(&self) -> Duration {
        Duration::from_secs(self.drain_timeout_secs.unwrap_or(DEFAULT_DRAIN_SECS))
    }
}

/// Parses and validates a controller config document.
///
/// # Errors
/// Returns [`ConfigError`] (with `file` set) for bad YAML, unknown keys,
/// mistyped values and empty paths; the error path names the key.
pub fn parse_controller(file: &str, text: &str) -> Result<ControllerConfig, ConfigError> {
    let cfg: ControllerConfig = parse_strict(file, text)?;
    for (key, value) in [("socket", &cfg.socket), ("state_dir", &cfg.state_dir)] {
        if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
            return Err(ConfigError::new(key, "must not be empty").in_file(file));
        }
    }
    if cfg.settle_every_secs == Some(0) {
        return Err(ConfigError::new("settle_every_secs", "must be above 0").in_file(file));
    }
    if cfg.web.is_some() {
        return Err(ConfigError::new(
            "web",
            "was removed from the controller config; run the separate moor-web daemon for the web UI",
        )
        .in_file(file));
    }
    if cfg.slack.is_some() {
        return Err(ConfigError::new(
            "slack",
            "was removed from the controller config; delete it, the separate moor-slack daemon is the Slack front",
        )
        .in_file(file));
    }
    if let Some(bad) = cfg.fronts.keys().find(|ns| !valid_front_namespace(ns)) {
        return Err(ConfigError::new(
            format!("fronts.{bad}"),
            "namespace must match [a-z][a-z0-9-]* and not be `local`",
        )
        .in_file(file));
    }
    if let Some((ns, _)) = cfg
        .fronts
        .iter()
        .find(|(_, f)| f.command.as_ref().is_some_and(Vec::is_empty))
    {
        return Err(
            ConfigError::new(format!("fronts.{ns}.command"), "must name a program").in_file(file),
        );
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests;
