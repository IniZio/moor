//! Arguments of `ask`.

use serde::{Deserialize, Serialize};

use crate::name::ForgeRepo;

/// Arguments of `ask`: a question about some repos, answered by the caller's idle place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskReq {
    /// The repos the question is about, the task's own first; at least one.
    pub repos: Vec<ForgeRepo>,
    /// The person's words as typed.
    pub prompt: String,
}
