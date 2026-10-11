//! The per-person config file (`$XDG_CONFIG_HOME/moor/config.yaml`), S3 subset.

use std::path::PathBuf;

use serde::Deserialize;

use super::ConfigError;

/// Provider used when `box.provider` is unset (old moor parity).
const DEFAULT_PROVIDER: &str = "local-container";
/// herdr session used when `surface.herdr.session` is unset.
const DEFAULT_HERDR_SESSION: &str = "moor";

/// `box:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BoxConfig {
    /// Box provider id.
    pub provider: Option<String>,
    /// Box size class.
    pub size: Option<String>,
    /// Cap on live Boxes.
    pub max_boxes: Option<u32>,
}

/// `surface.herdr:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HerdrConfig {
    /// herdr session name.
    pub session: Option<String>,
}

/// `surface:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SurfaceConfig {
    /// herdr settings.
    pub herdr: HerdrConfig,
}

/// `tools:` section: absolute or PATH-relative program overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
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

/// The user config; unknown keys are ignored until S4 makes it strict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    /// Box settings.
    #[serde(rename = "box")]
    pub box_: BoxConfig,
    /// Surface settings.
    pub surface: SurfaceConfig,
    /// Tool path overrides.
    pub tools: ToolsConfig,
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

    /// The configured Box provider, else `local-container`.
    #[must_use]
    pub fn provider_or_default(&self) -> &str {
        self.provider().unwrap_or(DEFAULT_PROVIDER)
    }
}

/// Parses user config text. An empty document yields defaults.
///
/// # Errors
/// Returns [`ConfigError::Parse`] for malformed YAML or mistyped values.
pub fn parse_user(text: &str) -> Result<UserConfig, ConfigError> {
    if text.trim().is_empty() {
        return Ok(UserConfig::default());
    }
    serde_yaml_ng::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))
}
