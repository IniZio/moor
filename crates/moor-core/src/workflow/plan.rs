//! The plan a run walks. Every workflow has one; until a person writes one it is
//! [`Plan::single`], a single step that delivers the prompt.
//!
//! [`Plan::parse`] is the only way to build a plan from text, so every plan in memory is valid.

use std::fmt;

use sha2::{Digest as _, Sha256};

use super::WorkflowError;
use crate::artifact::valid_name;

mod brief;
mod doc;
mod parse;

pub use brief::CheckSays;
pub use doc::{PlanDoc, PlanError, StepDoc};

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

    /// The hash as 64 lowercase hex digits.
    #[must_use]
    pub fn hex(&self) -> String {
        use std::fmt::Write as _;
        self.0.iter().fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    }

    /// The hash of 64 hex digits; none for any other text.
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut out = [0_u8; 32];
        for (slot, pair) in out.iter_mut().zip(bytes.chunks(2)) {
            let digits = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(digits, 16).ok()?;
        }
        Some(Self(out))
    }
}

/// How often a step may be entered in one run: 1 to 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tries(u8);

impl Tries {
    /// One entry, no retry.
    pub const ONE: Self = Self(1);

    /// The most entries a step may have.
    pub const MAX: u8 = 5;

    /// `n` tries, when it is 1 to 5.
    #[must_use]
    pub const fn new(n: u8) -> Option<Self> {
        if n >= 1 && n <= Self::MAX {
            Some(Self(n))
        } else {
            None
        }
    }

    /// The count.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A file the agent leaves under `.moor/artifacts/`.
///
/// It is an artifact name, or a prefix ending in `*` that one or more artifacts must match, such
/// as `before-*`. Artifacts are flat, so there are no directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceName(String);

impl EvidenceName {
    /// Validates a name or a `prefix*`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let stem = s.strip_suffix('*').unwrap_or(s);
        valid_name(stem).then(|| Self(s.to_owned()))
    }

    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One or more evidence names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// The first name, so the list is never empty.
    first: EvidenceName,
    /// The rest.
    rest: Vec<EvidenceName>,
}

impl Evidence {
    /// The names, from `names`; none when it is empty.
    #[must_use]
    pub fn new(names: Vec<EvidenceName>) -> Option<Self> {
        let mut names = names.into_iter();
        Some(Self {
            first: names.next()?,
            rest: names.collect(),
        })
    }

    /// Every name.
    pub fn names(&self) -> impl Iterator<Item = &EvidenceName> {
        std::iter::once(&self.first).chain(&self.rest)
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

/// Where a run goes after a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Next {
    /// After a pass: report ok, gate passed, exit 0, or yes.
    pub ok: StepId,
    /// After a fail: report fail, exit non-zero, or no. None ends the run as failed.
    pub fail: Option<StepId>,
}

/// What a step asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    /// The agent works in the Box.
    Work {
        /// What to do.
        instructions: String,
        /// How the agent knows it is done.
        done_when: String,
        /// The files that show it.
        evidence: Evidence,
        /// Where the run goes after it.
        next: Next,
    },
    /// The controller runs `command` in the Box. The exit code decides.
    Check {
        /// The shell command.
        command: String,
        /// Where the run goes after it.
        next: Next,
    },
    /// The run waits for the workflow's owner to say yes or no.
    Approve {
        /// What the owner is asked.
        question: String,
        /// Where the run goes after the answer.
        next: Next,
    },
    /// The agent delivers: a pull request, a message, a file, whatever the prompt asks. Its
    /// report's note is the run's result. A deliver step ends the run.
    Deliver {
        /// What to do.
        instructions: String,
        /// How the agent knows it is done.
        done_when: String,
    },
}

impl StepKind {
    /// The onward edges; a deliver step has none.
    #[must_use]
    pub const fn next(&self) -> Option<&Next> {
        match self {
            Self::Work { next, .. } | Self::Check { next, .. } | Self::Approve { next, .. } => {
                Some(next)
            }
            Self::Deliver { .. } => None,
        }
    }

    /// The kind as the wire names it.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Work { .. } => "work",
            Self::Check { .. } => "check",
            Self::Approve { .. } => "approve",
            Self::Deliver { .. } => "deliver",
        }
    }
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

    /// The step called `id`.
    #[must_use]
    pub fn step(&self, id: &StepId) -> Option<&Step> {
        self.steps().find(|s| &s.id == id)
    }

    /// How many steps.
    #[must_use]
    pub const fn step_count(&self) -> usize {
        1 + self.rest.len()
    }

    /// Whether this is the plan of a workflow nobody planned, as [`Plan::single`] makes it from
    /// `prompt`.
    #[must_use]
    pub fn is_single_of(&self, prompt: &str) -> bool {
        *self == Self::single(prompt)
    }

    /// The id of the one step of [`Plan::single`], which a run that never froze a plan reads as
    /// its only step.
    #[must_use]
    pub fn single_id() -> StepId {
        StepId(SINGLE_ID.to_owned())
    }
}
