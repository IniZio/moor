//! The controller process config file: strict parse plus validation, no IO.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::blobs::BlobsConfig;
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
    /// Seconds between the passes that close Boxes nobody needs any more; 0 turns them off.
    /// None means 60.
    pub auto_close_every_secs: Option<u64>,
    /// Seconds after a workflow run ends before its Box closes, counted from the last time a
    /// person opened its terminal or steered it; 0 keeps these Boxes. None means 86400.
    pub auto_close_run_after_secs: Option<u64>,
    /// Seconds a one-off Box (not from a run) may sit idle before it closes; 0 turns the idle
    /// close off. None means 604800.
    pub auto_close_idle_after_secs: Option<u64>,
    /// Fronts that each get a socket bound to their namespace, keyed by that namespace.
    pub fronts: BTreeMap<String, FrontEntry>,
    /// Where the ledger is also served on TCP with TLS, for a shared egress Proxy, machine runner
    /// or front in another pod; none serves it only on its unix socket.
    pub ledger_tls: Option<LedgerTls>,
    /// Where artifacts, reports and transcripts live; the state directory by default.
    pub blobs: BlobsConfig,
    /// The GitHub App the operator created for this install; none turns every App feature off.
    pub github_app: Option<GitHubAppConfig>,
    /// The users who may have an agent place, which is also the switch: with no one listed, no
    /// place starts for anyone.
    pub agent_places: AgentPlaces,
}

/// The `agent_places` entry: who gets a place, a repo-less Box the agent waits in before any task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentPlaces {
    /// The emails of the users on the list.
    pub users: Vec<String>,
}

/// The `github_app` entry: the id and key files of a GitHub App the operator created on GitHub.
/// moor only reads with it: installations, pull requests and collaborator permissions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubAppConfig {
    /// The App id GitHub shows on the App's settings page.
    pub id: u64,
    /// The PEM private key GitHub generated for the App; `~` allowed.
    pub private_key_file: PathBuf,
    /// The file holding the App's webhook secret; `~` allowed. Without it the gate's
    /// `/hooks/github` takes no deliveries.
    #[serde(default)]
    pub webhook_secret_file: Option<PathBuf>,
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
    /// Whether the front may claim and report the users' webhook deliveries.
    #[serde(default)]
    pub webhooks: bool,
    /// Whether the front may report a step for the agent in a Box, naming the Box from its own
    /// transport. Only a gate behind the Proxy should have it.
    #[serde(default)]
    pub step_reports: bool,
    /// Whether the front may run `exec_in_box` and `start_box` for the agent in a claimed agent
    /// place, naming the place from its own transport. Only a gate behind the Proxy should have
    /// it.
    #[serde(default)]
    pub box_tools: bool,
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

/// When and how the controller closes Boxes nobody needs any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoClose {
    /// Time between passes.
    pub every: Duration,
    /// How long after its run ends, or after the last steer or terminal, a run's Box closes;
    /// none keeps them.
    pub run_after: Option<Duration>,
    /// How long a one-off Box may sit idle before it closes; none turns that close off. A one-off Box
    /// whose pull request merged or closed closes whatever this is.
    pub idle_after: Option<Duration>,
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

/// Seconds a `setup_failed` Box is kept before the controller closes it when the config does not say.
const DEFAULT_CLOSE_SETUP_FAILED_SECS: u64 = 86_400;

/// Seconds between auto-close passes.
const DEFAULT_AUTO_CLOSE_EVERY_SECS: u64 = 60;

/// Seconds a run's Box stays open after the run ends.
const DEFAULT_AUTO_CLOSE_RUN_SECS: u64 = 86_400;

/// Seconds a one-off Box may sit idle.
const DEFAULT_AUTO_CLOSE_IDLE_SECS: u64 = 604_800;

/// Seconds a drain waits when the config does not say: long enough for a `start` to finish.
const DEFAULT_DRAIN_SECS: u64 = 30;

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

    /// The namespaces of the fronts that report steps for the agents in Boxes.
    #[must_use]
    pub fn step_report_fronts(&self) -> BTreeSet<String> {
        self.fronts
            .iter()
            .filter(|(_, front)| front.step_reports)
            .map(|(ns, _)| ns.clone())
            .collect()
    }

    /// The namespaces of the fronts that run the Box tools for the agents in claimed places.
    #[must_use]
    pub fn box_tool_fronts(&self) -> BTreeSet<String> {
        self.fronts
            .iter()
            .filter(|(_, front)| front.box_tools)
            .map(|(ns, _)| ns.clone())
            .collect()
    }

    /// The emails of the users who may have an agent place, lowercased.
    #[must_use]
    pub fn agent_place_users(&self) -> BTreeSet<String> {
        self.agent_places
            .users
            .iter()
            .map(|email| email.trim().to_lowercase())
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

    /// How the controller closes Boxes nobody needs any more; none when it closes none.
    #[must_use]
    pub fn auto_close(&self) -> Option<AutoClose> {
        let secs = |v: Option<u64>, default: u64| {
            Some(v.unwrap_or(default))
                .filter(|s| *s > 0)
                .map(Duration::from_secs)
        };
        Some(AutoClose {
            every: secs(self.auto_close_every_secs, DEFAULT_AUTO_CLOSE_EVERY_SECS)?,
            run_after: secs(self.auto_close_run_after_secs, DEFAULT_AUTO_CLOSE_RUN_SECS),
            idle_after: secs(
                self.auto_close_idle_after_secs,
                DEFAULT_AUTO_CLOSE_IDLE_SECS,
            ),
        })
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
    if let Some((key, why)) = cfg.blobs.problem() {
        return Err(ConfigError::new(key, why).in_file(file));
    }
    if let Some(bad) = cfg.fronts.keys().find(|ns| !valid_front_namespace(ns)) {
        return Err(ConfigError::new(
            format!("fronts.{bad}"),
            "namespace must match [a-z][a-z0-9-]* and not be `local`",
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
