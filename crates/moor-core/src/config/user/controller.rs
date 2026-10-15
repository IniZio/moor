//! The `controller:` section of the user config.

use std::path::PathBuf;

use serde::Deserialize;

use crate::config::ConfigError;

/// `controller:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ControllerClientConfig {
    /// Socket of a controller daemon; when set, CLI verbs run there instead of in-process.
    pub socket: Option<PathBuf>,
    /// SSH destination (`user@host` or a `~/.ssh/config` alias) whose `moor controller attach` reaches the controller.
    pub ssh: Option<String>,
}

impl ControllerClientConfig {
    /// `socket` is not blank; `ssh` is a plain destination; only one is set.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self
            .socket
            .as_ref()
            .is_some_and(|s| s.as_os_str().is_empty())
        {
            return Err(ConfigError::new("controller.socket", "must not be empty"));
        }
        if let Some(ssh) = &self.ssh {
            if ssh.is_empty() {
                return Err(ConfigError::new("controller.ssh", "must not be empty"));
            }
            if ssh.starts_with('-') {
                return Err(ConfigError::new(
                    "controller.ssh",
                    "must not start with '-' (ssh would read it as an option)",
                ));
            }
            if ssh.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(ConfigError::new(
                    "controller.ssh",
                    "must not contain whitespace or control characters",
                ));
            }
            if self.socket.is_some() {
                return Err(ConfigError::new(
                    "controller",
                    "set only one of socket and ssh",
                ));
            }
        }
        Ok(())
    }
}
