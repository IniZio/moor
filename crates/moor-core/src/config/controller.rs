//! The controller process config file: strict parse plus validation, no IO.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::proxy::front_rules;
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
    /// The machine runner's own state directory, the runner's `--state-dir`; `~` allowed. The
    /// host paths the runner's `box-mirror` and `box-put` name must lie inside it. None means
    /// `<state dir>/machine-runner`.
    pub runner_state_dir: Option<String>,
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
    /// The one directory the front may write, an absolute path; without it the front writes
    /// nothing.
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
    /// Absolute paths the front may read even where the controller hides the host from it.
    #[serde(default)]
    pub read_only: Vec<PathBuf>,
    /// Names of the rules the front's Proxy serves to the users it leases for, such as `claude`.
    #[serde(default)]
    pub proxy_rules: Vec<String>,
    /// TCP ports a client on the host reaches the front on at `127.0.0.1`, where the front
    /// listens on the same port inside its own network namespace.
    #[serde(default)]
    pub ports: Vec<u16>,
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

/// The name the controller's log gives the front Proxy, which no front may take.
const PROXY_LOG_NAME: &str = "proxy";

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
    for (key, value) in [
        ("socket", &cfg.socket),
        ("state_dir", &cfg.state_dir),
        ("runner_state_dir", &cfg.runner_state_dir),
    ] {
        if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
            return Err(ConfigError::new(key, "must not be empty").in_file(file));
        }
    }
    if let Some(dir) = cfg.runner_state_dir.as_deref() {
        if !(dir.starts_with('/') || dir.starts_with("~/")) {
            return Err(ConfigError::new(
                "runner_state_dir",
                "must be absolute or start with `~/`",
            )
            .in_file(file));
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
    if cfg.fronts.contains_key(PROXY_LOG_NAME) {
        return Err(ConfigError::new(
            format!("fronts.{PROXY_LOG_NAME}"),
            "is reserved: the controller logs the front Proxy as `front proxy`",
        )
        .in_file(file));
    }
    for (ns, front) in &cfg.fronts {
        front_entry(front).map_err(|(key, why)| {
            ConfigError::new(format!("fronts.{ns}.{key}"), why).in_file(file)
        })?;
    }
    Ok(cfg)
}

/// Checks one `fronts` entry; an error names the key and what is wrong with it.
fn front_entry(front: &FrontEntry) -> Result<(), (&'static str, &'static str)> {
    if front.command.as_ref().is_some_and(Vec::is_empty) {
        return Err(("command", "must name a program"));
    }
    let known = front_rules();
    if !front
        .proxy_rules
        .iter()
        .all(|r| known.iter().any(|k| k.name == *r))
    {
        return Err((
            "proxy_rules",
            "must name rules a front Proxy serves, such as claude",
        ));
    }
    if front.ports.contains(&0) {
        return Err(("ports", "must hold ports from 1 to 65535"));
    }
    if front.read_only.iter().any(|p| !p.is_absolute()) {
        return Err(("read_only", "must hold absolute paths"));
    }
    if front.read_only.iter().any(|p| p.starts_with("/tmp")) {
        return Err((
            "read_only",
            "must not be or lie under /tmp, which is the front's own tmp dir",
        ));
    }
    match &front.state_dir {
        Some(dir) if !dir.is_absolute() => Err(("state_dir", "must be an absolute path")),
        Some(dir) if dir.starts_with("/tmp") => Err((
            "state_dir",
            "must not lie under /tmp, which is the front's own tmp dir",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
