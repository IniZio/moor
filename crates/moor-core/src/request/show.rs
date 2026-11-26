//! Arguments of `show`.

use serde::{Deserialize, Serialize};

use super::BoxTarget;

/// Arguments of `show`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct ShowReq {
    /// The Box, in any state: by name, the newest Box of that name; all open Boxes when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "box")]
    pub target: Option<BoxTarget>,
    /// Number of events to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<u32>,
    /// Include agent state.
    #[serde(default)]
    pub agent: bool,
    /// Include reports.
    #[serde(default)]
    pub reports: bool,
    /// Include the latest version of each artifact.
    #[serde(default)]
    pub artifacts: bool,
    /// Include pane contents.
    #[serde(default)]
    pub pane: bool,
    /// Place the agent's saved transcript in the timeline: each tool call and message. Left out
    /// when false, so a controller that predates it still reads every other `show`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transcript: bool,
    /// Read every user's Boxes, not only the caller's: a list holds each user's open Boxes and
    /// the Box named may be another user's. It reaches no pane and grants no write.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub everyone: bool,
}
