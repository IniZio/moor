//! Port forwards over a machine's link.
//!
//! The supervisor asks the runner for a Box's ports and holds that connection, the carrier. The
//! runner asks the agent to listen, and each connection the agent accepts on the machine's
//! loopback comes back as a stream the agent opens. The runner names it on the carrier, and the
//! supervisor dials back once for it and pipes it to its own tunnel to the Box port.

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
    /// The Box ports, each served at the same number on the machine's loopback.
    pub ports: Vec<u16>,
}

/// The runner's answer to a [`Forward`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "reply")]
pub enum Forwarding {
    /// The agent listens on every port; the runner closes the connection once it stops.
    Up,
    /// Nothing listens.
    Refused {
        /// Why.
        reason: String,
    },
}

/// A line the runner sends on the carrier once the forwards are up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Carried {
    /// The machine accepted connection `n` on Box port `port`; it waits for a dial back.
    Conn {
        /// The connection's number, which the dial back names.
        n: u64,
        /// The Box port it came in on.
        port: u16,
    },
}

/// The first line on the runner's forward socket on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Local {
    /// A Box's forwards.
    Forward(Forward),
    /// A dial back for a connection a carrier named.
    Claim(Claim),
}

/// A dial back on this host for the connection a [`Carried::Conn`] named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Claim {
    /// Connection `n`.
    Conn {
        /// Its number.
        n: u64,
    },
}

/// The first line of a stream the agent opens: a connection it accepted for a listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    /// The listen's id, as its request gave it.
    pub listen: u64,
    /// The machine port the connection came in on.
    pub port: u16,
}

#[cfg(test)]
mod tests {
    use super::{Carried, Claim, Forward, Local};

    #[test]
    fn a_carried_connection_and_its_local_dial_back_read_as_one_line_each() {
        let line = serde_json::to_string(&Carried::Conn { n: 4, port: 8081 }).unwrap();
        assert_eq!(line, r#"{"conn":{"n":4,"port":8081}}"#);
        let dial: Local = serde_json::from_str(r#"{"conn":{"n":4}}"#).unwrap();
        assert_eq!(dial, Local::Claim(Claim::Conn { n: 4 }));
        let forward: Local =
            serde_json::from_str(r#"{"box_id":7,"machine_id":3,"ports":[8081]}"#).unwrap();
        let want = Forward {
            box_id: 7,
            machine_id: 3,
            ports: vec![8081],
        };
        assert_eq!(forward, Local::Forward(want));
    }
}
