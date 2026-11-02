//! Requests that take a Box or a gate.

use serde::{Deserialize, Serialize};

use super::BoxTarget;
use crate::name::BoxName;

/// Arguments of `close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseReq {
    /// The Box: by name, the newest open Box of that name, else the newest closed one.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Close despite unfinished work.
    #[serde(default)]
    pub force: bool,
}

/// Arguments of `close-failed`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseFailedReq {
    /// Close despite unfinished work.
    #[serde(default)]
    pub force: bool,
}

/// Arguments of commands that take only a box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: BoxName,
}

/// Arguments of `gate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateReq {
    /// Commit sha.
    pub sha: String,
    /// Gate name.
    pub name: String,
    /// Command to run for the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}
