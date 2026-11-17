//! What an agent reports for a step, and what the code gate asks of it.

use super::StepId;

/// The longest note a report may carry, in bytes. A deliver step's note is the run's result.
pub const NOTE_MAX: usize = 2048;

/// The largest evidence file the code gate accepts, in bytes.
pub const EVIDENCE_MAX: u64 = 25 * 1024 * 1024;

/// How the agent says a step went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepResult {
    /// The step is done; its evidence is in `.moor/artifacts/`.
    Ok,
    /// The step cannot be done, and the note says why.
    Fail,
}

impl StepResult {
    /// The result as the tool names it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Fail => "fail",
        }
    }
}

/// What the agent sends with `report_step`. The run and the Box come from the transport, never
/// from an argument.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReport {
    /// The step reported.
    pub step: StepId,
    /// How it went.
    pub result: StepResult,
    /// The artifact names the agent left; the step's evidence is what the gate checks.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// At most [`NOTE_MAX`] bytes; a deliver step's note is the run's result.
    #[serde(default)]
    pub note: String,
}
