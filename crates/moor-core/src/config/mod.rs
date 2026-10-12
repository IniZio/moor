//! User and repo configuration: pure parsing, no IO.

pub mod check;
mod user;

pub use user::{parse_user, BoxConfig, HerdrConfig, SurfaceConfig, ToolsConfig, UserConfig};

/// A config document that failed to parse or validate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The YAML is malformed or a value has the wrong type.
    #[error("config: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests;
