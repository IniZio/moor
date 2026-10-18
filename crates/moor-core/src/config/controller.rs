//! The controller process config file: strict parse plus validation, no IO.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::{parse_repo, parse_strict, ConfigError};

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
    /// Largest proof file, in bytes, the controller copies to its state dir; must be above 0.
    /// None means 104857600 (100 MB).
    pub proof_max_bytes: Option<u64>,
    /// Loopback-only web UI; None means it is not served.
    pub web: Option<WebConfig>,
    /// Slack Socket Mode app; None means it is not started.
    pub slack: Option<SlackConfig>,
    /// Repos the controller may clone on its own host, keyed by name.
    pub repos: RepoTable,
}

/// A repo the controller can clone and start Tasks in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    /// Clone URL, handed to the host's own `git clone`.
    pub url: String,
    /// Branch new Tasks start from; None means the repo's default branch.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// `.moor.yaml` to use when the repo has none.
    #[serde(default)]
    pub moor_yaml: Option<MoorYamlSource>,
}

/// Where a repo's fallback `.moor.yaml` text comes from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoorYamlSource {
    /// A file on the controller host.
    Path(String),
    /// The YAML text itself.
    Inline(String),
}

/// Known repos by name.
pub type RepoTable = BTreeMap<String, RepoEntry>;

/// Finds an entry by exact name, else by url (a trailing `/` and `.git` are ignored).
#[must_use]
pub fn find_repo<'a>(repos: &'a RepoTable, key: &str) -> Option<(&'a str, &'a RepoEntry)> {
    if let Some((name, entry)) = repos.get_key_value(key) {
        return Some((name.as_str(), entry));
    }
    let want = normalize_url(key);
    repos
        .iter()
        .find(|(_, e)| normalize_url(&e.url) == want)
        .map(|(name, e)| (name.as_str(), e))
}

/// Drops a trailing `/` and `.git` so equivalent clone URLs compare equal.
fn normalize_url(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    url.strip_suffix(".git")
        .unwrap_or(url)
        .trim_end_matches('/')
}

/// True when the url embeds a password or token. Urls reach clients inside
/// clone errors, so secrets must not live in them. A bare user (`git@host`)
/// is allowed for ssh forms only.
fn url_has_credentials(url: &str) -> bool {
    let url = url.trim();
    if let Some((scheme, rest)) = url.split_once("://") {
        let authority = rest.split('/').next().unwrap_or("");
        let Some((userinfo, _)) = authority.rsplit_once('@') else {
            return false;
        };
        let ssh = scheme.eq_ignore_ascii_case("ssh") || scheme.eq_ignore_ascii_case("git+ssh");
        return !ssh || userinfo.contains(':');
    }
    // scp-like `user:pass@host:path`
    url.split_once('@')
        .is_some_and(|(userinfo, _)| !userinfo.contains('/') && userinfo.contains(':'))
}

/// Checks every repo entry; error paths are `repos.<name>[.key]`.
fn validate_repos(file: &str, repos: &RepoTable) -> Result<(), ConfigError> {
    for (name, entry) in repos {
        let at = |leaf: &str| format!("repos.{name}{leaf}");
        let fail = |leaf: &str, why: &str| Err(ConfigError::new(at(leaf), why).in_file(file));
        if !crate::name::valid_repo_name(name) {
            return fail("", "name must match [A-Za-z0-9][A-Za-z0-9._-]*");
        }
        if entry.url.trim().is_empty() {
            return fail(".url", "must not be empty");
        }
        if url_has_credentials(&entry.url) {
            return fail(
                ".url",
                "credentials must not be in the url; use the host's git credential setup",
            );
        }
        if let Some(b) = &entry.default_branch {
            if b.trim().is_empty() || b.starts_with('-') {
                return fail(
                    ".default_branch",
                    "must be non-empty and not start with '-'",
                );
            }
        }
        match &entry.moor_yaml {
            Some(MoorYamlSource::Path(p)) if p.trim().is_empty() => {
                return fail(".moor_yaml", "path must not be empty");
            }
            Some(MoorYamlSource::Inline(text)) => {
                parse_repo("", text).map_err(|e| {
                    let leaf = if e.path.is_empty() {
                        at(".moor_yaml")
                    } else {
                        at(&format!(".moor_yaml.{}", e.path))
                    };
                    ConfigError::new(leaf, e.reason).in_file(file)
                })?;
            }
            _ => {}
        }
    }
    Ok(())
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

/// Slack Socket Mode app settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackConfig {
    /// File holding the app-level token (`xapp-`); the file must be mode 0600.
    pub app_token_file: PathBuf,
    /// File holding the bot token (`xoxb-`).
    pub bot_token_file: PathBuf,
    /// Channel ids that may start Tasks, such as `C0123ABC`; must not be empty.
    pub channels: Vec<String>,
    /// User ids that may start Tasks, such as `U0123ABC`; must not be empty.
    pub users: Vec<String>,
    /// Slack Web API base URL without a trailing `/`; `https://` or loopback `http://` only.
    #[serde(default = "default_slack_api_base")]
    pub api_base: String,
}

/// The public Slack Web API.
fn default_slack_api_base() -> String {
    "https://slack.com/api".to_owned()
}

/// Checks the `slack:` section; the error path names the key.
fn validate_slack(file: &str, slack: &SlackConfig) -> Result<(), ConfigError> {
    let bad = |path: &str, reason: &str| Err(ConfigError::new(path, reason).in_file(file));
    for (key, path) in [
        ("slack.app_token_file", &slack.app_token_file),
        ("slack.bot_token_file", &slack.bot_token_file),
    ] {
        if path.as_os_str().is_empty() {
            return bad(key, "must not be empty");
        }
    }
    for (key, ids) in [
        ("slack.channels", &slack.channels),
        ("slack.users", &slack.users),
    ] {
        if ids.is_empty() {
            return bad(key, "must list at least one id");
        }
        if ids.iter().any(|id| id.trim().is_empty()) {
            return bad(key, "ids must not be empty");
        }
    }
    let base = slack.api_base.as_str();
    let scheme_ok = base.starts_with("https://")
        || base.starts_with("http://127.0.0.1:")
        || base.starts_with("http://localhost:");
    if !scheme_ok {
        return bad(
            "slack.api_base",
            "must start with https:// (or http://127.0.0.1: / http://localhost:)",
        );
    }
    if base.ends_with('/') {
        return bad("slack.api_base", "must not end with /");
    }
    Ok(())
}

/// Seconds between settle passes when the config does not say.
const DEFAULT_SETTLE_SECS: u64 = 5;

/// Largest proof file the controller copies when the config does not say.
pub const DEFAULT_PROOF_MAX_BYTES: u64 = 100 * 1024 * 1024;

impl ControllerConfig {
    /// The largest proof file, in bytes, that is copied.
    #[must_use]
    pub fn proof_max_bytes(&self) -> u64 {
        self.proof_max_bytes.unwrap_or(DEFAULT_PROOF_MAX_BYTES)
    }

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
    if cfg.proof_max_bytes == Some(0) {
        return Err(ConfigError::new("proof_max_bytes", "must be above 0").in_file(file));
    }
    if let Some(web) = &cfg.web {
        if web.port == 0 {
            return Err(ConfigError::new("web.port", "must be above 0").in_file(file));
        }
        if web.token_file.as_os_str().is_empty() {
            return Err(ConfigError::new("web.token_file", "must not be empty").in_file(file));
        }
    }
    if let Some(slack) = &cfg.slack {
        validate_slack(file, slack)?;
    }
    validate_repos(file, &cfg.repos)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests;
