//! Arguments of `gate-answer`.

use serde::{Deserialize, Serialize};

use super::BoxTarget;
use crate::workflow::Answer;

/// Arguments of `gate-answer`: the Box owner's yes or no to the gate that asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateAnswerReq {
    /// The Box: by name, the newest open Box of that name.
    #[serde(rename = "box")]
    pub target: BoxTarget,
    /// Yes passes the gate; no fails the try.
    pub answer: Answer,
    /// What to tell the agent along with the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The question this answers (a `progress` card's `needs.id`). The answer is refused when
    /// the gate asks something else by now, so a stale click never answers a later question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
}
