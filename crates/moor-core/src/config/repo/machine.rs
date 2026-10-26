//! `machine:` in `.moor.yaml`: the labels a user's machine must hold to be attached to the
//! Box, and the Box ports it must reach at the same numbers on its own loopback.

use serde::Deserialize;

use super::super::user::check_label;
use super::super::SyncConfig;
use super::ConfigError;

/// Most labels or ports one `machine:` section may name.
const MACHINE_LIST_MAX: usize = 16;

/// `machine:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoMachine {
    /// Labels the machine must all hold.
    pub labels: Vec<String>,
    /// Box ports the machine sees at the same numbers on its loopback.
    pub ports: Vec<u16>,
    /// What a sync to the machine leaves alone.
    pub sync: SyncConfig,
}

impl RepoMachine {
    /// Checks the labels and ports, naming the entry that fails.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.labels.is_empty() {
            return Err(ConfigError::new(
                "machine.labels",
                "name at least one label",
            ));
        }
        for (key, len) in [("labels", self.labels.len()), ("ports", self.ports.len())] {
            if len > MACHINE_LIST_MAX {
                return Err(ConfigError::new(
                    format!("machine.{key}"),
                    format!("at most {MACHINE_LIST_MAX} entries"),
                ));
            }
        }
        for (i, label) in self.labels.iter().enumerate() {
            check_label(label).map_err(|m| ConfigError::new(format!("machine.labels[{i}]"), m))?;
        }
        for (i, port) in self.ports.iter().enumerate() {
            if *port == 0 {
                return Err(ConfigError::new(
                    format!("machine.ports[{i}]"),
                    "port 0 is not a port",
                ));
            }
            if self.ports.iter().take(i).any(|p| p == port) {
                return Err(ConfigError::new(
                    format!("machine.ports[{i}]"),
                    format!("port {port} is listed twice"),
                ));
            }
        }
        self.sync.validate("machine.sync")
    }
}
