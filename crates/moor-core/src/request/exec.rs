//! The `exec` request.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `exec`: run a command in the Box with its stdin, stdout and stderr streamed
/// as raw bytes over the connection, without a terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Command and arguments; a single word is run as a shell line.
    pub argv: Vec<String>,
}
