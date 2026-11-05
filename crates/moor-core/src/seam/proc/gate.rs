//! Marker-bounded stderr: show only what the user command wrote.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::ProcSpec;

/// Marker lines bounding the part of a streamed child's stderr that moor shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StderrGate {
    /// Line that opens the shown region; the line itself is not shown.
    pub begin: String,
    /// Line that closes the shown region; the line itself is not shown.
    pub end: String,
    /// A line inside the region that is never shown, only noted.
    pub hide: Option<Hidden>,
}

/// A whole stderr line moor reads for itself and never shows.
#[derive(Clone)]
pub struct Hidden {
    /// The line, matched whole.
    pub line: String,
    /// Set once the line was seen; clones share it.
    seen: Arc<AtomicBool>,
}

impl Hidden {
    /// Hides `line`, not yet seen.
    #[must_use]
    pub fn new(line: impl Into<String>) -> Self {
        Self {
            line: line.into(),
            seen: Arc::default(),
        }
    }

    /// Notes that the line was seen.
    pub fn note(&self) {
        self.seen.store(true, Ordering::SeqCst);
    }

    /// Whether the line was seen.
    #[must_use]
    pub fn seen(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for Hidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hidden")
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Hidden {
    fn eq(&self, other: &Self) -> bool {
        self.line == other.line
    }
}

impl Eq for Hidden {}

impl ProcSpec {
    /// Shows only stderr between the gate's marker lines.
    #[must_use]
    pub fn stderr_gate(mut self, gate: StderrGate) -> Self {
        self.stderr_gate = Some(gate);
        self
    }
}
