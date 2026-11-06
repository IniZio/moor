//! Requests that copy files between a Box and the host.

use serde::{Deserialize, Serialize};

use crate::name::{BoxId, BoxName};

/// Arguments of `cp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: BoxName,
    /// Source path.
    pub src: String,
    /// Destination path on the host, absolute.
    pub dest: String,
}

/// Arguments of `box-rsync`: the machine runner's rsync, run in the Box as `rsync --server`
/// with its stdio streamed both ways over the connection, as `exec` streams a command's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxRsyncReq {
    /// The Box's store id.
    #[serde(rename = "box")]
    pub id: BoxId,
    /// The command rsync hands its remote shell: `rsync --server …` naming one path in the
    /// Box's work tree.
    pub argv: Vec<String>,
}
