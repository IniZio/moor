//! Events about reaching into a Box from outside it.

use serde::Serialize;

/// Files copied out of a Box, a command run in it, or a port forwarded into it, by its user or
/// the operator.
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
    /// A person's port forward into the Box opened.
    PortForwardOpened {
        /// The port in the Box.
        port: u16,
        /// The port on the person's computer.
        local_port: u16,
    },
    /// A port forward into the Box ended, with what it carried.
    PortForwardClosed {
        /// The port in the Box.
        port: u16,
        /// Connections the forward carried.
        connections: u64,
        /// Bytes from the person's computer into the Box.
        bytes_in: u64,
        /// Bytes from the Box back to the person's computer.
        bytes_out: u64,
    },
    /// A request to forward a port into the Box was refused for a reason other than who asked.
    PortForwardRefused {
        /// The port in the Box.
        port: u16,
        /// Why, in words.
        reason: String,
    },
}

impl AccessEvent {
    /// The event's kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Copied { .. } => "copied",
            Self::ExecRan { .. } => "exec_ran",
            Self::PortForwardOpened { .. } => "port_forward_opened",
            Self::PortForwardClosed { .. } => "port_forward_closed",
            Self::PortForwardRefused { .. } => "port_forward_refused",
        }
    }
}
