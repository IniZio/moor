//! The controller process config file: strict parse plus validation, no IO.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::{parse_strict, ConfigError};

/// The `moor controller serve` config. Build it with [`parse_controller`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ControllerConfig {
    /// Unix socket path; `~` allowed. None means `<state dir>/controller.sock`.
    pub socket: Option<String>,
    /// moor state directory override; `~` allowed. None means the process default.
    pub state_dir: Option<String>,
    /// Seconds between passes that settle every open Task's agent state; must be above 0.
    /// None means 5.
    pub settle_every_secs: Option<u64>,
    /// Loopback-only web UI; None means it is not served.
    pub web: Option<WebConfig>,
}

/// The optional web UI section of the controller config.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    /// TCP port on the loopback interface; must be above 0.
    pub port: u16,
    /// File holding the bearer token; must not be empty.
    pub token_file: PathBuf,
}

/// Seconds between settle passes when the config does not say.
const DEFAULT_SETTLE_SECS: u64 = 5;

impl ControllerConfig {
    /// The time between settle passes.
    #[must_use]
    pub fn settle_every(&self) -> Duration {
        Duration::from_secs(self.settle_every_secs.unwrap_or(DEFAULT_SETTLE_SECS))
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
    if let Some(web) = &cfg.web {
        if web.port == 0 {
            return Err(ConfigError::new("web.port", "must be above 0").in_file(file));
        }
        if web.token_file.as_os_str().is_empty() {
            return Err(ConfigError::new("web.token_file", "must not be empty").in_file(file));
        }
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests;
