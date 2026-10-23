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
    /// Slack Web API base URL without a trailing `/`; `https://` or loopback `http://` only.
    #[serde(default = "default_api_base")]
    pub api_base: String,
    /// GitHub API base URL without a trailing `/`, where a pasted `gh auth token` is checked;
    /// `https://` or loopback `http://` only.
    #[serde(default = "default_github_api_base")]
    pub github_api_base: String,
    /// The coordinator agent; a missing section means every default.
    #[serde(default)]
    pub coordinator: Coordinator,
}

/// The `coordinator` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coordinator {
    /// The agent that coordinates each thread; `claude` is the only one.
    #[serde(default)]
    pub agent: CoordinatorAgent,
    /// Absolute path of the agent's binary; for `claude`, Claude Code 2.1.304 or newer.
    /// Without it the agent looks for its own binary on `PATH`.
    #[serde(default)]
    pub binary: Option<PathBuf>,
    /// The model each turn runs on.
    #[serde(default = "default_model")]
    pub model: String,
    /// Most KiB of context a turn gets; earlier messages fill what the rest leaves.
    #[serde(default = "default_context_kib")]
    pub context_kib: u32,
    /// Most Boxes a thread has open at once.
    #[serde(default = "default_max_open_boxes")]
    pub max_open_boxes: u32,
    /// Quiet minutes after which a thread's host Session ends.
    #[serde(default = "default_idle_minutes")]
    pub idle_minutes: u32,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self {
            agent: CoordinatorAgent::default(),
            binary: None,
            model: default_model(),
            context_kib: default_context_kib(),
            max_open_boxes: default_max_open_boxes(),
            idle_minutes: default_idle_minutes(),
        }
    }
}

/// The agents that can coordinate a thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoordinatorAgent {
    /// Claude Code.
    #[default]
    Claude,
}

/// The model a coordinator turn runs on unless the config names one.
fn default_model() -> String {
    "sonnet".to_owned()
}

/// The context budget in KiB unless the config names one.
const fn default_context_kib() -> u32 {
    32
}

/// The open-Box limit of a thread unless the config names one.
const fn default_max_open_boxes() -> u32 {
    1
}

/// The quiet minutes before a thread's host Session ends unless the config names them.
const fn default_idle_minutes() -> u32 {
    30
}

/// The public Slack Web API.
fn default_api_base() -> String {
    "https://slack.com/api".to_owned()
}

/// The public GitHub API.
fn default_github_api_base() -> String {
    "https://api.github.com".to_owned()
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
    if cfg.channels.is_empty() {
        return Err(("channels", "must list at least one id"));
    }
    if cfg.channels.iter().any(|id| id.trim().is_empty()) {
        return Err(("channels", "ids must not be empty"));
    }
    base_ok("api_base", &cfg.api_base)?;
    base_ok("github_api_base", &cfg.github_api_base)?;
    let c = &cfg.coordinator;
    if c.binary.as_ref().is_some_and(|b| !b.is_absolute()) {
        return Err(("coordinator.binary", "must be an absolute path"));
    }
    if c.model.trim().is_empty() {
        return Err(("coordinator.model", "must not be empty"));
    }
    if c.context_kib == 0 {
        return Err(("coordinator.context_kib", "must be at least 1"));
    }
    if c.max_open_boxes == 0 {
        return Err(("coordinator.max_open_boxes", "must be at least 1"));
    }
    if c.idle_minutes == 0 {
        return Err(("coordinator.idle_minutes", "must be at least 1"));
    }
    Ok(())
}

/// Checks that `base`, the value of `key`, is an `https://` URL or a loopback `http://` one
/// without a trailing `/`.
fn base_ok(key: &'static str, base: &str) -> Result<(), (&'static str, &'static str)> {
    let scheme_ok = base.starts_with("https://")
        || base.starts_with("http://127.0.0.1:")
        || base.starts_with("http://localhost:");
    if !scheme_ok {
        return Err((
            key,
            "must start with https:// (or http://127.0.0.1: / http://localhost:)",
        ));
    }
    if base.ends_with('/') {
        return Err((key, "must not end with /"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
