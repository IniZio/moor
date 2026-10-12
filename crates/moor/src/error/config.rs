//! Conversion from config errors to the command error.

use super::CommandError;
use moor_core::config::ConfigError;
use moor_core::ErrorKind;

impl From<ConfigError> for CommandError {
    /// A bad config file is a plain failure; the text is `file:key.path: reason`.
    fn from(e: ConfigError) -> Self {
        Self::new(ErrorKind::Failed, e.to_string())
    }
}
