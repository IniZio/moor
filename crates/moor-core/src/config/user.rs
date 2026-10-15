//! The per-person config file (`$XDG_CONFIG_HOME/moor/config.yaml`).
//!
//! Strict: unknown keys fail parsing; [`parse_user`] then validates values.

mod creds;
mod github;

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::proxy::ProxyConfig;
use super::{check, parse_strict, ConfigError};
pub use creds::{CredentialSource, McpServer};
pub use github::DEFAULT_YOU_TOKEN_CMD;

/// herdr session used when `surface.herdr.session` is unset.
const DEFAULT_HERDR_SESSION: &str = "moor";

/// Idle time before a Box sleeps when `box.sleep_after` is unset.
pub const DEFAULT_SLEEP_AFTER: Duration = Duration::from_mins(30);

/// Label shown for [`DEFAULT_SLEEP_AFTER`].
const DEFAULT_SLEEP_AFTER_LABEL: &str = "30m";

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

/// Which GitHub identity acts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Identity {
    /// The person's own token.
    You,
    /// The GitHub App.
    Bot,
    /// Pick by availability.
    #[default]
    Auto,
}

impl Identity {
    /// The lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Bot => "bot",
            Self::Auto => "auto",
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
}

impl BoxConfig {
    /// Idle time before a Box sleeps: the configured value or the default.
    #[must_use]
    pub fn sleep_after(&self) -> Duration {
        self.sleep_after
            .as_deref()
            .and_then(|t| parse_duration(t).ok())
            .unwrap_or(DEFAULT_SLEEP_AFTER)
    }

    /// The idle time as text: as configured (trimmed), or `30m` when unset.
    #[must_use]
    pub fn sleep_after_label(&self) -> String {
        self.sleep_after.as_deref().map_or_else(
            || DEFAULT_SLEEP_AFTER_LABEL.to_owned(),
            |t| t.trim().to_owned(),
        )
    }

    /// `sleep_after` parses as a duration.
    fn validate(&self) -> Result<(), ConfigError> {
        if let Some(t) = &self.sleep_after {
            parse_duration(t).map_err(|why| ConfigError::new("box.sleep_after", why))?;
        }
        Ok(())
    }
}

/// Parses `<positive integer><s|m|h|d>`, e.g. `45m`.
fn parse_duration(text: &str) -> Result<Duration, &'static str> {
    const MSG: &str = "must be a positive duration like 30s, 45m, 2h or 1d";
    let text = text.trim();
    let unit = text.chars().next_back().ok_or(MSG)?;
    let secs_per = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        _ => return Err(MSG),
    };
    let digits = text.strip_suffix(unit).ok_or(MSG)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(MSG);
    }
    let n: u64 = digits.parse().map_err(|_| "duration is too large")?;
    if n == 0 {
        return Err("must be greater than zero");
    }
    n.checked_mul(secs_per)
        .map(Duration::from_secs)
        .ok_or("duration is too large")
}

/// `drivers.<name>:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DriverConfig {
    /// Cap on live Boxes for this provider (at least 1).
    pub max_boxes: Option<NonZeroU32>,
}

/// `github:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GithubConfig {
    /// Which identity acts.
    pub identity: Identity,
    /// The GitHub App moor mints tokens from.
    pub app: Option<GithubApp>,
    /// Host command (argv) that prints a GitHub token; `gh auth token` when unset.
    pub you_token_cmd: Option<Vec<String>>,
    /// Environment variable names passed through to the token command.
    pub pass_env: Vec<String>,
}

/// `github.app:` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubApp {
    /// GitHub App id.
    pub id: u64,
    /// Private key file: absolute or `~/…`.
    pub key_path: PathBuf,
    /// Installation settings.
    #[serde(default)]
    pub install: BTreeMap<String, serde_json::Value>,
}

impl GithubApp {
    /// `key_path` is not blank.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.key_path.as_os_str().is_empty() {
            return Err(ConfigError::new("github.app.key_path", "must not be empty"));
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
    /// Named credential sources.
    pub credentials: BTreeMap<String, CredentialSource>,
    /// MCP servers.
    pub mcp_servers: BTreeMap<String, McpServer>,
    /// Box settings.
    #[serde(rename = "box")]
    pub box_: BoxConfig,
    /// Dotfiles source.
    pub dotfiles: Option<DotfilesConfig>,
    /// GitHub settings.
    pub github: GithubConfig,
    /// Agent settings.
    pub agent: AgentConfig,
    /// Surface settings.
    pub surface: SurfaceConfig,
    /// Tool path overrides.
    pub tools: ToolsConfig,
    /// Proxy tool rules.
    pub proxy: ProxyConfig,
    /// Credential names exposed to host-run box commands.
    pub box_credentials: Vec<String>,
    /// State directory override.
    pub state_dir: Option<PathBuf>,
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

    /// Checks values the types cannot. Error paths are dotted key paths.
    ///
    /// # Errors
    /// Returns the first invalid value found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(app) = &self.github.app {
            app.validate()?;
        }
        self.github.validate()?;
        self.box_.validate()?;
        creds::validate_credentials(&self.credentials)?;
        creds::validate_mcp(&self.mcp_servers, &self.credentials)?;
        if let Some(d) = &self.dotfiles {
            d.validate()?;
        }
        self.proxy.validate(&self.credentials)?;
        check::box_credentials(&self.box_credentials)
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
