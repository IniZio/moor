//! Where moor stands in making a workflow's plan. A person writes the prompt; moor reads it and
//! keeps a plan of its steps. A new prompt asks for a plan, a short Box makes it as `plan.json`,
//! and only that Box's plan is ever stored.

use serde::{Deserialize, Serialize};

use super::PromptHash;

/// A plan moor is making, or failed to make, for a workflow's prompt. Kept on the workflow until
/// its plan is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDraft {
    /// The hash of the prompt the plan is made from, as 64 hex digits.
    pub prompt: String,
    /// The Box making the plan, once moor asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_name: Option<String>,
    /// When moor asked for the Box, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Why no plan was made. moor does not try again until the prompt changes or the owner asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

impl PlanDraft {
    /// A plan wanted for `prompt`, with no Box asked for yet.
    #[must_use]
    pub fn wanted(prompt: &str) -> Self {
        Self {
            prompt: PromptHash::of(prompt).hex(),
            box_name: None,
            since: None,
            failed: None,
        }
    }

    /// Whether this draft is for `prompt`.
    #[must_use]
    pub fn is_for(&self, prompt: &str) -> bool {
        self.prompt == PromptHash::of(prompt).hex()
    }

    /// This draft, made by the Box `name` asked for at `at`.
    #[must_use]
    pub fn asked(&self, name: &str, at: i64) -> Self {
        Self {
            box_name: Some(name.to_owned()),
            since: Some(at),
            ..self.clone()
        }
    }

    /// This draft, ended without a plan for `why`.
    #[must_use]
    pub fn failed(&self, why: impl Into<String>) -> Self {
        Self {
            failed: Some(why.into()),
            ..self.clone()
        }
    }
}
