//! The front's config file: strict parse plus validation, no IO.

use std::path::PathBuf;

use moor_core::config::{parse_strict, ConfigError};
use serde::Deserialize;

/// The `moor-slack` config. Build it with [`parse_config`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// File holding the app-level token (`xapp-`); the file must be mode 0600.
    pub app_token_file: PathBuf,
    /// File holding the bot token (`xoxb-`); the file must be mode 0600.
    pub bot_token_file: PathBuf,
    /// Channel ids that may start Boxes, such as `C0123ABC`; must not be empty.
    pub channels: Vec<String>,
    /// User ids that may start Boxes, such as `U0123ABC`; must not be empty.
    pub users: Vec<String>,
    /// Slack Web API base URL without a trailing `/`; `https://` or loopback `http://` only.
    #[serde(default = "default_api_base")]
    pub api_base: String,
}

/// The public Slack Web API.
fn default_api_base() -> String {
    "https://slack.com/api".to_owned()
}

/// Parses and validates a front config document.
///
/// # Errors
/// Returns [`ConfigError`] (with `file` set) for bad YAML, unknown keys, mistyped values,
/// empty token paths or id lists, and an unusable API base; the error path names the key.
pub fn parse_config(file: &str, text: &str) -> Result<Config, ConfigError> {
    let cfg: Config = parse_strict(file, text)?;
    validate(&cfg).map_err(|(key, reason)| ConfigError::new(key, reason).in_file(file))?;
    Ok(cfg)
}

/// Checks the parsed values; the error names the key and the reason.
fn validate(cfg: &Config) -> Result<(), (&'static str, &'static str)> {
    for (key, path) in [
        ("app_token_file", &cfg.app_token_file),
        ("bot_token_file", &cfg.bot_token_file),
    ] {
        if path.as_os_str().is_empty() {
            return Err((key, "must not be empty"));
        }
    }
    for (key, ids) in [("channels", &cfg.channels), ("users", &cfg.users)] {
        if ids.is_empty() {
            return Err((key, "must list at least one id"));
        }
        if ids.iter().any(|id| id.trim().is_empty()) {
            return Err((key, "ids must not be empty"));
        }
    }
    let base = cfg.api_base.as_str();
    let scheme_ok = base.starts_with("https://")
        || base.starts_with("http://127.0.0.1:")
        || base.starts_with("http://localhost:");
    if !scheme_ok {
        return Err((
            "api_base",
            "must start with https:// (or http://127.0.0.1: / http://localhost:)",
        ));
    }
    if base.ends_with('/') {
        return Err(("api_base", "must not end with /"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
