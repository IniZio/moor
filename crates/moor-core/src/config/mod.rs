//! User and repo configuration: pure parsing, no IO.

pub mod check;
pub mod controller;
pub mod proxy;
pub mod repo;
pub mod settings;
mod user;

pub use controller::{
    find_repo, parse_controller, ControllerConfig, MoorYamlSource, RepoEntry, RepoTable,
    SlackConfig, DEFAULT_PROOF_MAX_BYTES,
};
pub use repo::{parse_repo, RepoBox, RepoCompose, RepoConfig};

pub use user::{
    parse_user, AgentConfig, BoxConfig, BoxSize, ControllerClientConfig, CredentialSource,
    DotfilesConfig, DriverConfig, GithubApp, GithubConfig, HerdrConfig, Identity, McpServer,
    SurfaceConfig, ToolsConfig, UserConfig, DEFAULT_SLEEP_AFTER, DEFAULT_YOU_TOKEN_CMD,
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
    let mut opts = serde_saphyr::Options::default();
    opts.strict_booleans = true;
    opts.no_schema = true;
    opts.ignore_binary_tag_for_string = true;
    opts.with_snippet = false;
    // Syntax first, so a malformed document is never reported as a type error.
    serde_saphyr::from_str_with_options::<serde::de::IgnoredAny>(text, opts.clone())
        .map_err(|e| ConfigError::new("", format!("invalid yaml: {e}")).in_file(file))?;
    let mut path = String::new();
    serde_saphyr::with_deserializer_from_str_with_options(text, opts, |de| {
        serde_path_to_error::deserialize::<_, T>(de).map_err(|e| {
            path = if e.path().iter().next().is_none() {
                String::new()
            } else {
                e.path().to_string()
            };
            e.into_inner()
        })
    })
    .map_err(|e| {
        let opts = serde_saphyr::RenderOptions::new(&NoLocation);
        ConfigError::new(path, e.render_with_options(opts)).in_file(file)
    })
}

/// Renders serde-saphyr messages without the `at line L, column C` suffix.
struct NoLocation;

impl serde_saphyr::Localizer for NoLocation {
    fn attach_location<'a>(
        &self,
        base: std::borrow::Cow<'a, str>,
        _: serde_saphyr::Location,
    ) -> std::borrow::Cow<'a, str> {
        base
    }
}

impl serde_saphyr::MessageFormatter for NoLocation {
    fn localizer(&self) -> &dyn serde_saphyr::Localizer {
        self
    }

    fn format_message<'a>(&self, err: &'a serde_saphyr::Error) -> std::borrow::Cow<'a, str> {
        serde_saphyr::DefaultMessageFormatter.format_message(err)
    }
}

#[cfg(test)]
mod tests;
