//! Port forwards over a machine's link.
//!
//! The supervisor asks the runner on its local socket, the runner asks the agent to listen,
//! and each connection the agent accepts on the machine's loopback comes back as a stream the
//! agent opens.

use serde::{Deserialize, Serialize};

/// The runner's forward socket, beside the runner's socket to the controller.
pub const SOCKET: &str = "machine-forwards.sock";
/// Most ports one forward may carry.
pub const MAX_PORTS: usize = 64;

/// The supervisor's request, the one line it sends before holding the connection open; closing
/// it ends the forwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    /// The Box whose ports these are.
    pub box_id: i64,
    /// The enrolled machine that serves them.
    pub machine_id: i64,
    /// `(box_port, host_port)`: the machine's loopback `box_port` reaches the host's loopback
    /// `host_port`, the driver's tunnel into the Box.
    pub ports: Vec<(u16, u16)>,
}

/// The runner's answer to a [`Forward`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "reply", deny_unknown_fields)]
pub enum Forwarding {
    /// The agent listens on every port; the runner closes the connection once it stops.
    Up,
    /// Nothing listens.
    Refused {
        /// Why.
        reason: String,
    },
}

/// The first line of a stream the agent opens: a connection it accepted for a listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accepted {
    /// The listen's id, as its request gave it.
    pub listen: u64,
    /// The machine port the connection came in on.
    pub port: u16,
}
