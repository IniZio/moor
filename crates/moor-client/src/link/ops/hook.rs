//! The hooks a machine's owner may keep in its hooks dir.

use serde::{Deserialize, Serialize};

/// A Box lifecycle hook, a file of that name in the agent's hooks dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hook {
    /// Before a Box's first call that needs its facts; its last stdout line is the facts.
    Prepare,
    /// When a prepared Box falls asleep.
    Suspend,
    /// Once the Box is no longer open.
    Release,
}

impl Hook {
    /// Its file's name.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Suspend => "suspend",
            Self::Release => "release",
        }
    }
}
