//! An output as the run and task pages list it.

use serde::{Deserialize, Serialize};

/// One output of a Box: something its agent made outside moor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputView {
    /// What it is, such as `github.comment`.
    pub kind: String,
    /// Where it lives.
    pub url: String,
    /// The one line that names it.
    pub title: String,
    /// Milliseconds since the epoch of its first report.
    pub first_at: i64,
    /// Milliseconds since the epoch of its latest report.
    pub last_at: i64,
}

/// Value of `output-report`: what the agent is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputReportValue {
    /// In words for the agent.
    pub reply: String,
}
