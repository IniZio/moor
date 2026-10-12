//! User and repo configuration: pure parsing, no IO.

pub mod check;
pub mod egress;
pub mod repo;
pub mod settings;
mod user;

pub use repo::{parse_repo, RepoBox, RepoCompose, RepoConfig};

pub use user::{
    parse_user, AgentConfig, BoxConfig, BoxSize, CredentialSource, DotfilesConfig, DriverConfig,
    EgressConfig, GitIdentity, GithubApp, GithubConfig, HerdrConfig, Identity, McpServer,
    SurfaceConfig, ToolsConfig, UserConfig,
};

pub use settings::{CredPlan, Flags, Layer, Sourced, TaskSettings};

use std::fmt;

/// A config document that failed to parse or validate.
///
/// Displays as `file:path: reason`; empty parts are dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ConfigError {
    /// Source file name; empty when not yet known.
    pub file: String,
    /// Dotted key path of the offending key; empty for the whole document.
    pub path: String,
    /// What is wrong.
    pub reason: String,
}

impl ConfigError {
    /// An error at `path` with no file yet.
    pub fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            file: String::new(),
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// Sets the file name, keeping path and reason.
    #[must_use]
    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        self.file = file.into();
        self
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let loc = match (self.file.is_empty(), self.path.is_empty()) {
            (true, true) => return f.write_str(&self.reason),
            (false, true) => self.file.clone(),
            (true, false) => self.path.clone(),
            (false, false) => format!("{}:{}", self.file, self.path),
        };
        write!(f, "{loc}: {}", self.reason)
    }
}

/// Strict parse of a YAML document. An empty, whitespace-only or `null`
/// document yields `T::default()`.
///
/// Error paths are dotted keys; sequence indices render as `[n]` appended to
/// the parent (`items[2].name`). An unknown field is reported with the
/// unknown key itself in the path. Invalid YAML syntax gives an empty path and
/// a reason starting `invalid yaml`.
///
/// # Errors
/// Returns [`ConfigError`] for malformed YAML, unknown keys or mistyped values.
pub fn parse_strict<T: serde::de::DeserializeOwned + Default>(
    file: &str,
    text: &str,
) -> Result<T, ConfigError> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)
        .map_err(|e| ConfigError::new("", format!("invalid yaml: {e}")).in_file(file))?;
    if value.is_null() {
        return Ok(T::default());
    }
    serde_path_to_error::deserialize(value).map_err(|e| {
        let mut path = e.path().to_string();
        if path == "." {
            path.clear();
        }
        let reason = e.inner().to_string();
        ConfigError::new(path, reason).in_file(file)
    })
}

#[cfg(test)]
mod tests;
