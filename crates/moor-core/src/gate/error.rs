//! Why gates or outcomes as written are not valid.

/// Why gates are not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// The text is not gates: not JSON, a field of the wrong type, or an unknown field.
    #[error("the gates are not valid: {0}")]
    Json(String),
    /// No gates.
    #[error("a Box with gates needs at least one gate")]
    NoGates,
    /// More than [`super::GATES_MAX`].
    #[error("{0} gates; use at most 8")]
    TooManyGates(usize),
    /// A name that is not 1-32 of `[a-z0-9-]` starting with a letter.
    #[error("gate `{gate}`: use 1-32 lowercase letters, digits or '-', starting with a letter")]
    BadName {
        /// The name as written.
        gate: String,
    },
    /// Two gates share a name.
    #[error("gate name `{0}` is used twice")]
    DuplicateName(String),
    /// Nothing to pass: no evidence, no checks and no approval.
    #[error("gate `{gate}` needs evidence, a check or an owner approval")]
    NothingToPass {
        /// The gate.
        gate: String,
    },
    /// An evidence name that is neither an artifact name nor a `prefix*`.
    #[error("gate `{gate}`: evidence `{name}` is not a flat file name or a `prefix*`")]
    BadEvidence {
        /// The gate.
        gate: String,
        /// What was written.
        name: String,
    },
    /// More evidence names than a report may list.
    #[error("gate `{gate}` lists {len} evidence names; use at most 32")]
    TooManyEvidence {
        /// The gate.
        gate: String,
        /// How many.
        len: usize,
    },
    /// More than [`super::CHECKS_MAX`] checks.
    #[error("gate `{gate}` has {len} checks; use at most 8")]
    TooManyChecks {
        /// The gate.
        gate: String,
        /// How many.
        len: usize,
    },
    /// A check with no command.
    #[error("gate `{gate}` has an empty check")]
    EmptyCheck {
        /// The gate.
        gate: String,
    },
    /// A check with a blank sentence.
    #[error("gate `{gate}` has a check whose `says` is empty")]
    EmptySays {
        /// The gate.
        gate: String,
    },
    /// A check's sentence over more than one line.
    #[error("gate `{gate}`: a check's `says` must be one line")]
    SaysNotOneLine {
        /// The gate.
        gate: String,
    },
    /// A check's sentence over [`super::SAYS_MAX`] characters.
    #[error("gate `{gate}`: a check says {len} characters; use at most 200")]
    SaysTooLong {
        /// The gate.
        gate: String,
        /// Its length.
        len: usize,
    },
    /// A check over [`super::CHECK_MAX`] bytes.
    #[error("gate `{gate}`: a check is {len} bytes; use at most 4096")]
    CheckTooLong {
        /// The gate.
        gate: String,
        /// Its length.
        len: usize,
    },
    /// A `max_tries` outside 1 to 5.
    #[error("gate `{gate}`: max_tries must be 1 to 5, not {got}")]
    MaxTries {
        /// The gate.
        gate: String,
        /// What was written.
        got: u64,
    },
    /// A gate named for the stage moor keeps after the last gate.
    #[error("gate name `{0}` is kept for the stage that decides the outcome; pick another")]
    ReservedName(String),
    /// More than [`super::OUTCOMES_MAX`] outcomes.
    #[error("{0} outcomes; use at most 6")]
    TooManyOutcomes(usize),
    /// An outcome name that is blank, over one line or over [`super::OUTCOME_NAME_MAX`] characters.
    #[error("outcome `{0}`: use one line of 1-40 characters")]
    BadOutcomeName(String),
    /// Two outcomes share a name.
    #[error("outcome name `{0}` is used twice")]
    DuplicateOutcome(String),
    /// Every outcome is `anytime`, so none settles a run whose gates all passed.
    #[error("at least one outcome must not be anytime")]
    OnlyAnytime,
    /// An `anytime` outcome with no check, which would end every run at once.
    #[error("outcome `{0}` is anytime and needs a check")]
    AnytimeWithoutCheck(String),
    /// An outcome with no check before the last outcome that is not `anytime`.
    #[error("outcome `{0}` has no check, so only the last outcome that is not anytime may")]
    OtherwiseNotLast(String),
    /// An outcome whose sentence or checks are not valid.
    #[error("outcome `{outcome}`: {why}")]
    BadOutcome {
        /// The outcome.
        outcome: String,
        /// What is wrong.
        why: String,
    },
}
