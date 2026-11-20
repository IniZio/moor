//! Arguments of `steer`.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Text to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Menu entry to pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<u32>,
    /// Send the agent's interrupt key instead of text or a menu entry.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt: bool,
    /// The prompt this answers (a `progress` card's `needs.id`). A menu choice is refused when
    /// the agent waits at another prompt by now, so a stale click never answers a later one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
}
