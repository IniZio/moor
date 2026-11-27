//! Where moor stands in reading a workflow's prompt. A person writes the prompt; moor reads it and
//! keeps its reading: a plan of steps, or gates a run must pass. A new prompt asks for a reading,
//! an agent place writes it as `plan.json`, and only that place's reading is ever stored.

use serde::{Deserialize, Serialize};

use super::{Plan, PromptHash};
use crate::gate::Gates;

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
    /// Whether the owner asked moor to replace gates a person wrote with its own reading. Without
    /// it, gates a person wrote stay and no reading is made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
    /// How many hand-ins moor refused from the Box reading the prompt. Left out at 0, so a draft
    /// without one keeps the JSON it had.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub refused: u32,
}

/// Whether `n` is 0.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
const fn is_zero(n: &u32) -> bool {
    *n == 0
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
            replace: false,
            refused: 0,
        }
    }

    /// This draft, replacing gates a person wrote when `replace` is true.
    #[must_use]
    pub fn replacing(self, replace: bool) -> Self {
        Self { replace, ..self }
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

    /// This draft, with one more refused hand-in counted.
    #[must_use]
    pub fn refused_again(&self) -> Self {
        Self {
            refused: self.refused.saturating_add(1),
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

/// What the Box that read a workflow's prompt left in `plan.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderLeft {
    /// Steps a run walks.
    Plan(Plan),
    /// Gates a run's Box must pass, with the outcomes it may end in.
    Gates(Gates),
}

impl ReaderLeft {
    /// What `text` holds, made for `prompt`: gates when it is an object with `gates`, a plan
    /// otherwise.
    ///
    /// # Errors
    /// Every reason the file is not valid.
    pub fn parse(text: &str, prompt: &str) -> Result<Self, Vec<String>> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| vec![format!("not JSON: {e}")])?;
        if value.get("gates").is_some() {
            return Gates::from_json(text)
                .map(|g| Self::Gates(g.read_from(prompt)))
                .map_err(|e| vec![e.to_string()]);
        }
        Plan::parse(text, prompt)
            .map(Self::Plan)
            .map_err(|errors| errors.iter().map(ToString::to_string).collect())
    }

    /// The hash of the prompt it was made from.
    #[must_use]
    pub const fn made_from(&self) -> Option<&PromptHash> {
        match self {
            Self::Plan(p) => Some(&p.made_from),
            Self::Gates(g) => g.made_from(),
        }
    }
}
