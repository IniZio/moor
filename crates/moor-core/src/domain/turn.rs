//! How a finished agent turn ended.

/// How a finished turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The agent finished and waits for input.
    Idle,
    /// The agent waits on an answer.
    NeedsInput,
    /// The agent's pane is gone.
    Exited,
}

impl TurnOutcome {
    /// The wire name of the outcome.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::NeedsInput => "needs_input",
            Self::Exited => "exited",
        }
    }
}
