//! The repo config file (`.moor.yaml`): strict parse plus validation, no IO.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{check, parse_strict, BoxSize, ConfigError};

/// `compose:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoCompose {
    /// Prefix for compose project names.
    pub project_prefix: Option<String>,
}

/// `box:` section; only `size`, `template` and `disk_gb` are allowed at the repo layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoBox {
    /// Box size class; repo > user > default.
    pub size: Option<BoxSize>,
    /// Box template name handed to the provider on acquire; unset keeps the provider default.
    pub template: Option<String>,
    /// Disk size in GB handed to the provider on acquire; the provider checks the range.
    pub disk_gb: Option<u32>,
}

/// The repo config. Build it with [`parse_repo`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoConfig {
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
            ("setup", self.setup != other.setup),
            ("teardown", self.teardown != other.teardown),
            ("env", self.env != other.env),
            ("compose", self.compose != other.compose),
            ("box", self.box_ != other.box_),
            (
                "box_credentials",
                self.box_credentials != other.box_credentials,
            ),
        ]
        .into_iter()
        .filter_map(|(key, changed)| changed.then_some(key))
        .collect()
    }

    /// Checks values the types cannot.
    fn validate(&self) -> Result<(), ConfigError> {
        non_blank("setup", &self.setup)?;
        non_blank("teardown", &self.teardown)?;
        self.validate_box_credentials()?;
        if self
            .box_
            .template
            .as_deref()
            .is_some_and(|t| t.trim().is_empty())
        {
            return Err(ConfigError::new(
                "box.template",
                "template must not be empty",
            ));
        }
        for name in self.env.keys() {
            let path = format!("env.{name}");
            check::env_name(name)
                .and_then(|()| check::not_reserved_env(name))
                .map_err(|r| ConfigError::new(path, r))?;
        }
        Ok(())
    }

    /// Checks each `box_credentials` name and rejects duplicates.
    fn validate_box_credentials(&self) -> Result<(), ConfigError> {
        check::box_credentials(&self.box_credentials)
            .map_err(|(i, m)| ConfigError::new(format!("box_credentials[{i}]"), m))
    }
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
