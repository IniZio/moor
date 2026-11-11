//! The port forward requests.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `forward-open`: a person asks to forward `port` of the Box to their computer.
/// The controller wakes a sleeping Box, checks that something listens, and records the forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardOpenReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// The port on the Box's loopback.
    pub port: u16,
    /// The port the person's computer listens on.
    pub local_port: u16,
}

/// Arguments of `forward`: carry one TCP connection to a port on the Box's loopback.
///
/// Its bytes go both ways raw over this connection until either side ends it. It takes over the
/// connection: the controller socket closes it after the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// The port on the Box's loopback.
    pub port: u16,
}

/// Arguments of `forward-close`: the person ended a forward, which carried these counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardCloseReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// The port on the Box's loopback.
    pub port: u16,
    /// Connections the forward carried.
    pub connections: u64,
    /// Bytes from the person's computer into the Box.
    pub bytes_in: u64,
    /// Bytes from the Box back to the person's computer.
    pub bytes_out: u64,
}
