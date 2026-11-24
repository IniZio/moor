//! Arguments of `run`.

use serde::{Deserialize, Serialize};

use crate::name::BoxName;

/// Arguments of `run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct RunReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: BoxName,
    /// Record the run under this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// Command and arguments.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Program output goes to moor's own stdout and stderr instead of the run log.
    /// Never read from or written to JSON.
    #[serde(skip)]
    pub inherit_output: bool,
}
