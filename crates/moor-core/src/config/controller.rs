//! The controller process config file: strict parse plus validation, no IO.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
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
    /// Seconds between passes that settle every open Box's agent state; must be above 0.
    /// None means 5.
    pub settle_every_secs: Option<u64>,
    /// Shortest gap, in seconds, a workflow's cron may have between two firings; must be above
    /// 0. None means 3600.
    pub workflow_min_interval_secs: Option<u64>,
    /// Seconds a SIGTERM waits for in-flight requests before the controller exits anyway.
    /// None means 600.
    pub drain_timeout_secs: Option<u64>,
    /// Seconds a Box may sit in `setup_failed` before the controller closes it; 0 turns the
    /// close off. None means 86400.
    pub close_setup_failed_after_secs: Option<u64>,
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
    /// Where the ledger is also served on TCP with TLS, for a shared egress Proxy, machine runner
    /// or front in another pod; none serves it only on its unix socket.
    pub ledger_tls: Option<LedgerTls>,
}

/// The `ledger_tls` entry: the ledger on TCP, behind the controller's certificate, answering
/// only clients whose certificate `client_ca` signed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerTls {
    /// The address to listen on, such as `0.0.0.0:7443`.
    pub listen: SocketAddr,
    /// The PEM certificate chain the controller presents; `~` allowed.
    pub cert: PathBuf,
    /// The PEM private key of `cert`; `~` allowed.
    pub key: PathBuf,
    /// The PEM CA bundle a client's certificate must chain to; `~` allowed.
    pub client_ca: PathBuf,
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
    /// Whether the front may claim and report the users' webhook deliveries.
    #[serde(default)]
    pub webhooks: bool,
    /// TCP ports a client on the host reaches the front on at `127.0.0.1`, where the front
    /// listens on the same port inside its own network namespace.
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Set when the front runs elsewhere and dials `ledger_tls`.
    #[serde(default)]
    pub remote: Option<RemoteFront>,
}

/// A front that runs elsewhere and opens each connection to `ledger_tls` with a hello.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteFront {
    /// The file holding the key the front's hello must present; `~` allowed.
    pub key_file: PathBuf,
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

/// Seconds a `setup_failed` Box is kept before the controller closes it when the config does not say.
const DEFAULT_CLOSE_SETUP_FAILED_SECS: u64 = 86_400;

/// Seconds a drain waits when the config does not say: long enough for a `start` to finish.
const DEFAULT_DRAIN_SECS: u64 = 600;

impl ControllerConfig {
    /// The time between settle passes.
    #[must_use]
    pub fn settle_every(&self) -> Duration {
        Duration::from_secs(self.settle_every_secs.unwrap_or(DEFAULT_SETTLE_SECS))
    }

    /// The namespaces of the fronts that take webhooks.
    #[must_use]
    pub fn webhook_fronts(&self) -> BTreeSet<String> {
        self.fronts
            .iter()
            .filter(|(_, front)| front.webhooks)
            .map(|(ns, _)| ns.clone())
            .collect()
    }

    /// The namespaces of the fronts that run elsewhere and dial `ledger_tls`.
    #[must_use]
    pub fn remote_fronts(&self) -> BTreeSet<String> {
        self.fronts
            .iter()
            .filter(|(_, front)| front.remote.is_some())
            .map(|(ns, _)| ns.clone())
            .collect()
    }

    /// The shortest gap a workflow's cron may have between two firings.
    #[must_use]
    pub fn workflow_min_interval(&self) -> Duration {
        self.workflow_min_interval_secs
            .map_or(crate::workflow::MIN_FIRING_INTERVAL, Duration::from_secs)
    }

    /// How long a Box may stay `setup_failed` before the controller closes it; None when closing
    /// is off.
    #[must_use]
    pub fn close_setup_failed_after(&self) -> Option<Duration> {
        let secs = self
            .close_setup_failed_after_secs
            .unwrap_or(DEFAULT_CLOSE_SETUP_FAILED_SECS);
        (secs > 0).then(|| Duration::from_secs(secs))
    }

    /// How long a SIGTERM waits for in-flight requests.
    #[must_use]
    pub fn drain_timeout(&self) -> Duration {
        Duration::from_secs(self.drain_timeout_secs.unwrap_or(DEFAULT_DRAIN_SECS))
    }
}

/// Refuses an interval key set to 0.
fn check_intervals(cfg: &ControllerConfig, file: &str) -> Result<(), ConfigError> {
    for (key, secs) in [
        ("settle_every_secs", cfg.settle_every_secs),
        ("workflow_min_interval_secs", cfg.workflow_min_interval_secs),
    ] {
        if secs == Some(0) {
            return Err(ConfigError::new(key, "must be above 0").in_file(file));
        }
    }
    Ok(())
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
    check_intervals(&cfg, file)?;
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
    if cfg.ledger_tls.is_none() {
        if let Some((ns, _)) = cfg.fronts.iter().find(|(_, f)| f.remote.is_some()) {
            return Err(ConfigError::new(
                format!("fronts.{ns}.remote"),
                "needs ledger_tls, which a remote front dials",
            )
            .in_file(file));
        }
    }
    Ok(cfg)
}

/// Checks one `fronts` entry; an error names the key and what is wrong with it.
fn front_entry(front: &FrontEntry) -> Result<(), (&'static str, &'static str)> {
    if front.command.as_ref().is_some_and(Vec::is_empty) {
        return Err(("command", "must name a program"));
    }
    if front.remote.is_some() && front.command.is_some() {
        return Err((
            "remote",
            "a remote front runs elsewhere, so it takes no command",
        ));
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
