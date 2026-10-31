//! The repo config file (`.moor.yaml`): strict parse plus validation, no IO.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::proxy::{default_rules, injected};
use super::{check, parse_strict, BoxSize, ConfigError};
use crate::cache::CacheConfig;

pub use machine::RepoMachine;

mod machine;

/// The most characters the `brief` of a repo config may hold.
pub const BRIEF_MAX_CHARS: usize = 4000;

/// `compose:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoCompose {
    /// Prefix for compose project names.
    pub project_prefix: Option<String>,
}

/// `box:` section; only `size` is allowed at the repo layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoBox {
    /// Box size class; repo > user > default.
    pub size: Option<BoxSize>,
}

/// `node:` section: what a node running the Box must offer. A repo never names a node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoNode {
    /// Labels the node must carry.
    pub labels: Vec<String>,
}

/// The repo config. Build it with [`parse_repo`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoConfig {
    /// Notes from the repo's author for the Box's agent.
    pub brief: Option<String>,
    /// Setup commands.
    pub setup: Vec<String>,
    /// Teardown commands.
    pub teardown: Vec<String>,
    /// Plain environment variables for the Box.
    pub env: BTreeMap<String, String>,
    /// `compose:` section.
    pub compose: RepoCompose,
    /// `box:` section.
    #[serde(rename = "box")]
    pub box_: RepoBox,
    /// Names of user credentials exposed on the Box.
    pub box_credentials: Vec<String>,
    /// `cache:` section.
    pub cache: CacheConfig,
    /// `machine:` section: what a user's machine attached to the Box must offer.
    pub machine: Option<RepoMachine>,
    /// `node:` section.
    pub node: RepoNode,
}

/// Parses and validates a `.moor.yaml` document.
///
/// # Errors
/// Returns [`ConfigError`] (with `file` set) for bad YAML, unknown
/// keys, and invalid values; the error path names the key.
pub fn parse_repo(file: &str, text: &str) -> Result<RepoConfig, ConfigError> {
    let cfg: RepoConfig = parse_strict(file, text)?;
    cfg.validate().map_err(|e| e.in_file(file))?;
    Ok(cfg)
}

impl RepoConfig {
    /// Top-level `.moor.yaml` keys whose values differ between `self` and `other`, in file order.
    #[must_use]
    pub fn changed_keys(&self, other: &Self) -> Vec<&'static str> {
        [
            ("brief", self.brief != other.brief),
            ("setup", self.setup != other.setup),
            ("teardown", self.teardown != other.teardown),
            ("env", self.env != other.env),
            ("compose", self.compose != other.compose),
            ("box", self.box_ != other.box_),
            (
                "box_credentials",
                self.box_credentials != other.box_credentials,
            ),
            ("cache", self.cache != other.cache),
            ("machine", self.machine != other.machine),
            ("node", self.node != other.node),
        ]
        .into_iter()
        .filter_map(|(key, changed)| changed.then_some(key))
        .collect()
    }

    /// Checks values the types cannot.
    fn validate(&self) -> Result<(), ConfigError> {
        self.validate_brief()?;
        non_blank("setup", &self.setup)?;
        non_blank("teardown", &self.teardown)?;
        self.validate_box_credentials()?;
        check_cache(&self.cache)?;
        self.machine
            .as_ref()
            .map_or(Ok(()), RepoMachine::validate)?;
        self.validate_node()?;
        for name in self.env.keys() {
            let path = format!("env.{name}");
            check::env_name(name)
                .and_then(|()| check::not_reserved_env(name))
                .map_err(|r| ConfigError::new(path, r))?;
        }
        Ok(())
    }

    /// Each `node.labels` entry is a valid label.
    fn validate_node(&self) -> Result<(), ConfigError> {
        for (i, label) in self.node.labels.iter().enumerate() {
            super::user::check_label(label)
                .map_err(|m| ConfigError::new(format!("node.labels[{i}]"), m))?;
        }
        Ok(())
    }

    /// Rejects a blank `brief` and one longer than [`BRIEF_MAX_CHARS`].
    fn validate_brief(&self) -> Result<(), ConfigError> {
        let Some(text) = &self.brief else {
            return Ok(());
        };
        if text.trim().is_empty() {
            return Err(ConfigError::new("brief", "brief must not be empty"));
        }
        let len = text.chars().count();
        if len > BRIEF_MAX_CHARS {
            return Err(ConfigError::new(
                "brief",
                format!("brief must be at most {BRIEF_MAX_CHARS} characters, found {len}"),
            ));
        }
        Ok(())
    }

    /// Checks each `box_credentials` name against the default rules, which alone a repo
    /// config knows, and rejects duplicates.
    fn validate_box_credentials(&self) -> Result<(), ConfigError> {
        check::box_credentials(&self.box_credentials, &injected(&default_rules()))
            .map_err(|(i, m)| ConfigError::new(format!("box_credentials[{i}]"), m))
    }
}

/// Checks a `cache:` section, naming the key below `cache` that fails.
pub(super) fn check_cache(cache: &CacheConfig) -> Result<(), ConfigError> {
    cache
        .check()
        .map_err(|(at, why)| ConfigError::new(format!("cache.{at}"), why))
}

/// Rejects blank entries of a command list.
fn non_blank(name: &str, items: &[String]) -> Result<(), ConfigError> {
    items
        .iter()
        .position(|s| s.trim().is_empty())
        .map_or(Ok(()), |i| {
            Err(ConfigError::new(
                format!("{name}[{i}]"),
                format!("{name} entry must not be empty"),
            ))
        })
}

#[cfg(test)]
mod tests;
