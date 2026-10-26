//! Events about the user's machine attached to a Box.

use serde::Serialize;

/// A machine attached to a Box, left out of it, or its forwards failing or ending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MachineEvent {
    /// The machine reaches the Box's ports at the same numbers on its own loopback.
    MachineAttached {
        /// The machine's name.
        name: String,
        /// The Box ports it reaches.
        ports: Vec<u16>,
    },
    /// The Box started without a machine.
    MachineLeftOut {
        /// The labels asked for.
        labels: Vec<String>,
        /// Why no machine was attached.
        reason: String,
    },
    /// The forwards to the machine did not open or died.
    MachineForwardFailed {
        /// The Box port whose tunnel failed; none when the ssh link failed.
        port: Option<u16>,
        /// What went wrong.
        error: String,
    },
    /// The machine was let go: the Box closed or slept.
    MachineDetached {
        /// The machine's name.
        name: String,
    },
}

impl MachineEvent {
    /// The `snake_case` kind name; equals the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::MachineAttached { .. } => "machine_attached",
            Self::MachineLeftOut { .. } => "machine_left_out",
            Self::MachineForwardFailed { .. } => "machine_forward_failed",
            Self::MachineDetached { .. } => "machine_detached",
        }
    }
}
