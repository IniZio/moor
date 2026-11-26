//! Arguments of `output-report`, which the gate sends for the agent in a Box.

use serde::{Deserialize, Serialize};

use crate::name::BoxId;

/// Arguments of `output-report`: something the Box's agent made outside moor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputReportReq {
    /// The Box whose agent reported. The gate takes it from `X-Moor-Box`, which only the Proxy
    /// sets, and never from the agent's arguments.
    pub box_id: BoxId,
    /// What it is, such as `github.comment`.
    pub kind: String,
    /// Where it lives, an `https` address.
    pub url: String,
    /// One line that names it.
    pub title: String,
}
