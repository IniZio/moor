//! Marker-bounded stderr: show only what the user command wrote.

use super::ProcSpec;

/// Marker lines bounding the part of a streamed child's stderr that moor shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StderrGate {
    /// Line that opens the shown region; the line itself is not shown.
    pub begin: String,
    /// Line that closes the shown region; the line itself is not shown.
    pub end: String,
}

impl ProcSpec {
    /// Shows only stderr between the gate's marker lines.
    #[must_use]
    pub fn stderr_gate(mut self, gate: StderrGate) -> Self {
        self.stderr_gate = Some(gate);
        self
    }
}
