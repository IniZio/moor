//! The `attach` request.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `attach`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Columns of the caller's terminal.
    pub cols: u16,
    /// Rows of the caller's terminal.
    pub rows: u16,
}
