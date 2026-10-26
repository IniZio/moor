//! Arguments of the machine requests: a user's own machines, reached over the host's ssh.

use serde::{Deserialize, Serialize};

use crate::user::Identity;

/// Arguments of `machine-add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineAddReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The machine's name.
    pub name: String,
    /// The ssh target, `user@host` or `user@host:port`.
    pub ssh: String,
    /// The machine's labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// How many Boxes may hold it at once, each in its own port slot.
    #[serde(default = "one")]
    pub capacity: u16,
}

const fn one() -> u16 {
    1
}

/// Arguments of `machine-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineRemoveReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The machine's name.
    pub name: String,
}
