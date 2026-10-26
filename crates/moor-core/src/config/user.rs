//! The per-person config file (`$XDG_CONFIG_HOME/moor/config.yaml`).
//!
//! Strict: unknown keys fail parsing; [`parse_user`] then validates values.

mod controller;
mod creds;
mod duration;
mod machine;
mod own_mcp;
mod service;

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::proxy::{injected, ProxyConfig};
use super::{check, parse_strict, ConfigError};
use crate::cache::CacheConfig;
pub use controller::ControllerClientConfig;
pub use creds::McpServer;
use duration::{duration_or, parse_duration};
pub(crate) use machine::check_label;
pub use machine::{check_ssh, pick_machine, wake_slot, MachinePick, UserMachine};
pub use own_mcp::{box_mcp, BoxMcp, UserMcp, DEFAULT_HEADER};
pub use service::{MachineService, MachinesConfig};

/// herdr session used when `surface.herdr.session` is unset.
const DEFAULT_HERDR_SESSION: &str = "moor";

/// Idle time before a Box sleeps when `box.sleep_after` is unset.
pub const DEFAULT_SLEEP_AFTER: Duration = Duration::from_mins(30);

/// Idle time before a Box waiting on a prompt sleeps when `box.sleep_after_blocked` is unset.
const DEFAULT_SLEEP_AFTER_BLOCKED: Duration = Duration::from_mins(5);

/// Awake time before any Box sleeps when `box.max_awake` is unset.
const DEFAULT_MAX_AWAKE: Duration = Duration::from_hours(2);

/// Label shown for [`DEFAULT_SLEEP_AFTER`].
const DEFAULT_SLEEP_AFTER_LABEL: &str = "30m";

/// Time an agent launch may take before it counts as timed out when `box.launch_timeout` is unset.
const DEFAULT_LAUNCH_TIMEOUT: Duration = Duration::from_mins(3);

/// Box size class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoxSize {
    /// Default size.
    #[default]
    Small,
    /// Medium.
    Medium,
    /// Large.
    Large,
    /// Extra large.
    Xlarge,
}

impl BoxSize {
    /// The lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::Xlarge => "xlarge",
        }
    }
}

/// `box:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BoxConfig {
    /// Box provider id (free string).
    pub provider: Option<String>,
    /// Box size class.
    pub size: Option<BoxSize>,
    /// Cap on live Boxes (at least 1).
    pub max_boxes: Option<NonZeroU32>,
    /// Idle time before a Box sleeps.
    pub sleep_after: Option<String>,
    /// Idle time before a Box waiting on a prompt or a first-run screen sleeps.
    pub sleep_after_blocked: Option<String>,
    /// Awake time before any Box sleeps.
    pub max_awake: Option<String>,
    /// Time an agent launch may take before the Box counts as quiet.
    pub launch_timeout: Option<String>,
}

impl BoxConfig {
    /// Time an agent launch may take: the configured value or the default.
    #[must_use]
    pub fn launch_timeout(&self) -> Duration {
        self.launch_timeout
            .as_deref()
            .and_then(|t| parse_duration(t).ok())
            .unwrap_or(DEFAULT_LAUNCH_TIMEOUT)
    }

    /// Idle time before a Box sleeps: the configured value or the default.
    #[must_use]
    pub fn sleep_after(&self) -> Duration {
        duration_or(self.sleep_after.as_deref(), DEFAULT_SLEEP_AFTER)
    }

    /// Idle time before a Box waiting on a prompt sleeps: the configured value or the default.
    #[must_use]
    pub fn sleep_after_blocked(&self) -> Duration {
        duration_or(
            self.sleep_after_blocked.as_deref(),
            DEFAULT_SLEEP_AFTER_BLOCKED,
        )
    }

    /// Awake time before any Box sleeps: the configured value or the default.
    #[must_use]
    pub fn max_awake(&self) -> Duration {
        duration_or(self.max_awake.as_deref(), DEFAULT_MAX_AWAKE)
    }

    /// The idle time as text: as configured (trimmed), or `30m` when unset.
    #[must_use]
    pub fn sleep_after_label(&self) -> String {
        self.sleep_after.as_deref().map_or_else(
            || DEFAULT_SLEEP_AFTER_LABEL.to_owned(),
            |t| t.trim().to_owned(),
        )
    }

    /// Each sleep timer parses as a duration.
    fn validate(&self) -> Result<(), ConfigError> {
        for (key, value) in [
            ("box.sleep_after", &self.sleep_after),
            ("box.sleep_after_blocked", &self.sleep_after_blocked),
            ("box.max_awake", &self.max_awake),
            ("box.launch_timeout", &self.launch_timeout),
        ] {
            if let Some(t) = value {
                parse_duration(t).map_err(|why| ConfigError::new(key, why))?;
            }
        }
        Ok(())
    }
}

/// `drivers.<name>:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DriverConfig {
    /// Cap on live Boxes for this provider (at least 1).
    pub max_boxes: Option<NonZeroU32>,
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

    /// Every `strip_env` entry is a valid variable name.
    fn validate(&self, provider: &str) -> Result<(), ConfigError> {
        for (i, name) in self.strip_env.iter().enumerate() {
            let here = format!("drivers.{provider}.strip_env[{i}]");
            check::env_name(name).map_err(|why| ConfigError::new(here, why))?;
        }
        Ok(())
    }
}

/// `agent:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentConfig {
    /// Default model.
    pub model: Option<String>,
    /// Whether moor installs the agent on a Box that lacks it; on by default.
    pub install: Option<bool>,
}

/// `dotfiles:` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotfilesConfig {
    /// Local directory: absolute or `~/…`.
    pub path: PathBuf,
    /// Install script, relative without `..`.
    pub install: Option<String>,
}

impl DotfilesConfig {
    /// Path well-formed; install script relative.
    fn validate(&self) -> Result<(), ConfigError> {
        if !self.path.is_absolute() && !self.path.starts_with("~") {
            return Err(ConfigError::new(
                "dotfiles.path",
                "path must be absolute or start with ~/",
            ));
        }
        if let Some(i) = &self.install {
            let p = Path::new(i);
            let bad = i.is_empty()
                || p.is_absolute()
                || p.components()
                    .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
            if bad {
                return Err(ConfigError::new(
                    "dotfiles.install",
                    "install must be a non-empty relative path without ..",
                ));
            }
        }
        Ok(())
    }
}

/// `surface.herdr:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HerdrConfig {
    /// herdr session name.
    pub session: Option<String>,
}

/// `surface:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SurfaceConfig {
    /// herdr settings.
    pub herdr: HerdrConfig,
}

/// `tools:` section: absolute or PATH-relative program overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ToolsConfig {
    /// `crabbox` program.
    pub crabbox: Option<PathBuf>,
    /// `herdr` program.
    pub herdr: Option<PathBuf>,
    /// `claude` program.
    pub claude: Option<PathBuf>,
    /// `gh` program.
    pub gh: Option<PathBuf>,
    /// `ssh` program.
    pub ssh: Option<PathBuf>,
    /// `git` program.
    pub git: Option<PathBuf>,
}

/// The user config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UserConfig {
    /// Per-provider limits.
    pub drivers: BTreeMap<String, DriverConfig>,
    /// Removed key, kept only so a stale config gets a pointed error; `None` in any config
    /// that validates.
    #[serde(default, deserialize_with = "super::controller::present_key")]
    #[allow(
        clippy::option_option,
        reason = "outer Some means the key was present, even when null"
    )]
    pub credentials: Option<Option<serde_json::Value>>,
    /// MCP servers.
    pub mcp_servers: BTreeMap<String, McpServer>,
    /// Box settings.
    #[serde(rename = "box")]
    pub box_: BoxConfig,
    /// Dotfiles source.
    pub dotfiles: Option<DotfilesConfig>,
    /// The service Boxes reach for the machine they hold.
    pub machines: MachinesConfig,
    /// Removed key, kept only so a stale config gets a pointed error; `None` in any config
    /// that validates.
    #[serde(default, deserialize_with = "super::controller::present_key")]
    #[allow(
        clippy::option_option,
        reason = "outer Some means the key was present, even when null"
    )]
    pub github: Option<Option<serde_json::Value>>,
    /// Agent settings.
    pub agent: AgentConfig,
    /// Surface settings.
    pub surface: SurfaceConfig,
    /// Tool path overrides.
    pub tools: ToolsConfig,
    /// Proxy tool rules.
    pub proxy: ProxyConfig,
    /// Controller daemon to run CLI verbs on.
    pub controller: ControllerClientConfig,
    /// Credential names exposed to host-run box commands.
    pub box_credentials: Vec<String>,
    /// State directory override.
    pub state_dir: Option<PathBuf>,
    /// Presets and user-scoped cache volumes for every Box.
    pub cache: CacheConfig,
}

impl UserConfig {
    /// The herdr session every adapter call selects; `moor` when unset.
    #[must_use]
    pub fn herdr_session(&self) -> &str {
        self.surface
            .herdr
            .session
            .as_deref()
            .unwrap_or(DEFAULT_HERDR_SESSION)
    }

    /// The configured Box provider, if any.
    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        self.box_.provider.as_deref()
    }

    /// The Proxy rules a Box gets: `proxy.rules` plus one per MCP server in `mcp` whose header
    /// names a secret.
    #[must_use]
    pub fn box_proxy(&self, mcp: &BTreeMap<String, McpServer>) -> ProxyConfig {
        let mut out = self.proxy.clone();
        for (name, server) in mcp {
            if let Some(rule) = server.proxy_rule() {
                out.rules.insert(creds::rule_name(name), rule);
            }
        }
        out
    }

    /// Checks values the types cannot. Error paths are dotted key paths.
    ///
    /// # Errors
    /// Returns the first invalid value found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (key, set) in [
            ("credentials", self.credentials.is_some()),
            ("github", self.github.is_some()),
        ] {
            if set {
                return Err(ConfigError::new(
                    key,
                    "was removed; each moor user stores their own secrets with `moor user secret set NAME`",
                ));
            }
        }
        self.box_.validate()?;
        for (name, d) in &self.drivers {
            d.validate(name)?;
        }
        let injected = injected(&self.proxy.merged_rules());
        creds::validate_mcp(&self.mcp_servers, &injected, &self.proxy.rules)?;
        if let Some(d) = &self.dotfiles {
            d.validate()?;
        }
        if let Some(s) = &self.machines.service {
            s.validate()?;
        }
        self.machines.sync.validate("machines.sync")?;
        self.proxy.validate()?;
        self.controller.validate()?;
        super::repo::check_cache(&self.cache)?;
        check::box_credentials(&self.box_credentials, &injected)
            .map_err(|(i, m)| ConfigError::new(format!("box_credentials[{i}]"), m))
    }
}

/// Parses and validates user config text. An empty document yields defaults.
///
/// # Errors
/// Returns [`ConfigError`] (with `file`) for malformed YAML, unknown keys,
/// mistyped or invalid values.
pub fn parse_user(file: &str, text: &str) -> Result<UserConfig, ConfigError> {
    let cfg: UserConfig = parse_strict(file, text)?;
    cfg.validate().map_err(|e| e.in_file(file))?;
    Ok(cfg)
}

#[cfg(test)]
mod tests;
