//! Arguments of `reading-hand-in`, which the gate sends for the Box reading an automation's
//! prompt.

use serde::{Deserialize, Serialize};

use crate::name::BoxId;
use crate::reading::HandIn;

/// Arguments of `reading-hand-in`: what the reader Box made of the prompt it reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadingHandInReq {
    /// The Box that hands it in. The gate takes it from `X-Moor-Box`, which only the Proxy sets,
    /// and never from the agent's arguments.
    pub box_id: BoxId,
    /// The phrases and effects it hands in.
    pub reading: HandIn,
}

/// What moor answers an accepted hand-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadingHandInValue {
    /// What to do next, in words for the reader.
    pub reply: String,
}
