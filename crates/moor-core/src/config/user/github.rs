//! `github.you_token_cmd` and `github.pass_env`: the host command that prints a token.

use std::collections::BTreeSet;

use super::GithubConfig;
use crate::config::{check, ConfigError};

/// Token command used when `github.you_token_cmd` is unset.
pub const DEFAULT_YOU_TOKEN_CMD: [&str; 3] = ["gh", "auth", "token"];

impl GithubConfig {
    /// The argv that prints a GitHub token: the configured one, else the default.
    #[must_use]
    pub fn token_cmd(&self) -> Vec<String> {
        self.you_token_cmd
            .clone()
            .unwrap_or_else(|| DEFAULT_YOU_TOKEN_CMD.map(String::from).to_vec())
    }

    /// Checks the token command and the variable names passed to it.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if let Some(cmd) = &self.you_token_cmd {
            if cmd.is_empty() || cmd.iter().any(String::is_empty) {
                return Err(ConfigError::new(
                    "github.you_token_cmd",
                    "must be a non-empty list of non-empty strings",
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for (i, name) in self.pass_env.iter().enumerate() {
            let here = format!("github.pass_env[{i}]");
            check::env_name(name).map_err(|why| ConfigError::new(here.clone(), why))?;
            if !seen.insert(name) {
                return Err(ConfigError::new(here, "listed twice"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
