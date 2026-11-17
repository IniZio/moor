//! The plan a run walks. Every workflow has one; until a person writes one it is
//! [`Plan::single`], a single step that delivers the prompt.
//!
//! This holds the shapes a run reads today. A step that checks or asks is added to
//! [`StepKind`] with the parser that makes it.

use std::fmt;

use sha2::{Digest as _, Sha256};

use super::WorkflowError;

/// A step's id: 1-32 bytes of `[a-z0-9-]`, starting with a letter. Stable across recompiles, so
/// history stays attached to the step it was written for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct StepId(String);

impl StepId {
    /// Validates an id.
    ///
    /// # Errors
    /// [`WorkflowError::StepId`] for any other text.
    pub fn parse(s: &str) -> Result<Self, WorkflowError> {
        let mut bytes = s.bytes();
        let ok = (1..=32).contains(&s.len())
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(WorkflowError::StepId(s.to_owned()))
        }
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StepId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The SHA-256 of the prompt a plan was made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptHash([u8; 32]);

impl PromptHash {
    /// The hash of `prompt`.
    #[must_use]
    pub fn of(prompt: &str) -> Self {
        Self(Sha256::digest(prompt.as_bytes()).into())
    }
}

/// How often a step may be entered in one run: 1 to 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tries(u8);

impl Tries {
    /// One entry, no retry.
    pub const ONE: Self = Self(1);

    /// The count.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A plan. The first step is where a run starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Where a run starts.
    first: Step,
    /// The steps after it.
    rest: Vec<Step>,
    /// Stale when it differs from the prompt's hash.
    pub made_from: PromptHash,
    /// Whether a person changed it after it was made.
    pub edited: bool,
}

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Its id.
    pub id: StepId,
    /// A short name, at most 120 bytes.
    pub title: String,
    /// How often a run may enter it.
    pub max_tries: Tries,
    /// What it asks.
    pub kind: StepKind,
}

/// What a step asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    /// The agent delivers: a pull request, a message, a file, whatever the prompt asks. Its
    /// report's note is the run's result. A deliver step ends the run.
    Deliver {
        /// What to do.
        instructions: String,
        /// How the agent knows it is done.
        done_when: String,
    },
}

/// The id of the one step of [`Plan::single`].
const SINGLE_ID: &str = "deliver";

impl Plan {
    /// The plan of a workflow nobody planned: one deliver step whose instructions are `prompt`.
    #[must_use]
    pub fn single(prompt: &str) -> Self {
        Self {
            first: Step {
                id: Self::single_id(),
                title: "Deliver".to_owned(),
                max_tries: Tries::ONE,
                kind: StepKind::Deliver {
                    instructions: prompt.to_owned(),
                    done_when: "The prompt's work is done.".to_owned(),
                },
            },
            rest: Vec::new(),
            made_from: PromptHash::of(prompt),
            edited: false,
        }
    }

    /// The step a run starts at.
    #[must_use]
    pub const fn first(&self) -> &Step {
        &self.first
    }

    /// Every step, the first one first.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        std::iter::once(&self.first).chain(&self.rest)
    }

    /// The id of the one step of [`Plan::single`], which a run that never froze a plan reads as
    /// its only step.
    #[must_use]
    pub fn single_id() -> StepId {
        StepId(SINGLE_ID.to_owned())
    }
}
