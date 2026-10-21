//! The controller process config file: strict parse plus validation, no IO.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

use super::{parse_repo, parse_strict, ConfigError};
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
    /// Repos the controller may clone on its own host, keyed by name.
    pub repos: RepoTable,
    /// Fronts that each get a socket bound to their namespace, keyed by that namespace.
    #[allow(
        clippy::zero_sized_map_values,
        reason = "`fronts: {slack: {}}` is the config shape; entries gain settings later"
    )]
    pub fronts: BTreeMap<String, FrontEntry>,
}

/// One front's entry under `fronts:`; it has no settings yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontEntry {}

/// Wraps whatever follows the key in `Some`, so a null value stays distinguishable from a missing key.
#[allow(
    clippy::option_option,
    reason = "outer Some means the key was present, even when null"
)]
fn present_key<'de, D>(d: D) -> Result<Option<Option<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<serde_json::Value>::deserialize(d).map(Some)
}

/// A repo the controller can clone and start Boxes in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    /// Clone URL, handed to the host's own `git clone`.
    pub url: String,
    /// Branch new Boxes start from; None means the repo's default branch.
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
    validate_repos(file, &cfg.repos)?;
    if let Some(bad) = cfg.fronts.keys().find(|ns| !valid_front_namespace(ns)) {
        return Err(ConfigError::new(
            format!("fronts.{bad}"),
            "namespace must match [a-z][a-z0-9-]* and not be `local`",
        )
        .in_file(file));
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests;
