//! Events about reaching into a Box from outside it.

use serde::Serialize;

/// Files copied out of a Box, or a command run in it, by its user or the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessEvent {
    /// Files were copied.
    Copied {
        /// The copy source.
        from: String,
        /// The copy destination.
        to: String,
    },
    /// A command ran in the Box through `exec` and ended.
    ExecRan {
        /// The command, as shell text.
        command: String,
        /// Its exit code; none when it was killed or the link dropped first.
        exit: Option<i32>,
        /// True when it ran past its timeout and was killed.
        timed_out: bool,
    },
}

impl AccessEvent {
    /// The event's kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Copied { .. } => "copied",
            Self::ExecRan { .. } => "exec_ran",
        }
    }
}
