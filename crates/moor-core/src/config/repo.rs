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

/// A macOS Box capability a repo can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoxCapability {
    /// Xcode toolchain (`xcode`).
    Xcode,
    /// iOS Simulator (`ios-simulator`).
    IosSimulator,
    /// `SwiftPM` build-tool plugins (`swiftpm-plugins`); implies `xcode`.
    SwiftpmPlugins,
}

/// `box.sandbox:` project paths, relative to the workdir.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoSandbox {
    /// Extra paths the sandbox may read.
    pub read: Vec<String>,
    /// Extra paths the sandbox may write.
    pub write: Vec<String>,
}

/// `box:` section: `size`, `capabilities` and `sandbox` are allowed at the repo layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoBox {
    /// Box size class; repo > user > default.
    pub size: Option<BoxSize>,
    /// macOS Box capabilities the repo needs.
    pub capabilities: Vec<BoxCapability>,
    /// Project paths added to the sandbox.
    pub sandbox: RepoSandbox,
}

impl RepoBox {
    /// Refuses `box.capabilities` on a Box that is not macOS.
    ///
    /// # Errors
    /// Returns [`ConfigError`] at `box.capabilities` when any capability is set and `macos` is false.
    pub fn check_platform(&self, macos: bool) -> Result<(), ConfigError> {
        if macos || self.capabilities.is_empty() {
            return Ok(());
        }
        Err(ConfigError::new(
            "box.capabilities",
            "box.capabilities applies only to a macOS Box; remove it or use a macOS Box",
        ))
    }

    /// Checks capability duplicates and sandbox paths.
    fn validate(&self) -> Result<(), ConfigError> {
        for (i, cap) in self.capabilities.iter().enumerate() {
            if self.capabilities.get(..i).is_some_and(|p| p.contains(cap)) {
                return Err(ConfigError::new(
                    format!("box.capabilities[{i}]"),
                    "list each capability once",
                ));
            }
        }
        sandbox_paths("read", &self.sandbox.read)?;
        sandbox_paths("write", &self.sandbox.write)
    }
}

/// Rejects sandbox paths that are empty, absolute, home-relative or climb out with `..`.
fn sandbox_paths(kind: &str, items: &[String]) -> Result<(), ConfigError> {
    items
        .iter()
        .position(|p| {
            p.trim().is_empty()
                || p.starts_with('/')
                || p.starts_with('~')
                || p.split('/').any(|c| c == "..")
        })
        .map_or(Ok(()), |i| {
            Err(ConfigError::new(
                format!("box.sandbox.{kind}[{i}]"),
                "a sandbox path must be inside the workdir; write it relative to the checkout",
            ))
        })
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
        self.box_.validate()?;
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
mod capability_tests;
#[cfg(test)]
mod tests;
