//! Per-invocation context handed to operations.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::seam::clock::Clock;
use moor_core::Warning;

/// State shared by one command run.
pub(crate) struct Cx {
    /// Resolved state directory.
    state_dir: Option<PathBuf>,
    /// Warnings recorded so far.
    warnings: RefCell<Vec<Warning>>,
    /// Time source; the only way ops read time.
    clock: Arc<dyn Clock>,
}

impl Cx {
    /// Create a context with an optional resolved state directory and a clock.
    pub(crate) const fn new(state_dir: Option<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        Self {
            state_dir,
            warnings: RefCell::new(Vec::new()),
            clock,
        }
    }

    /// Time source for this run.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Resolved state directory, if any.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
    pub(crate) fn state_dir(&self) -> Option<&Path> {
        self.state_dir.as_deref()
    }

    /// Record a warning for the final response.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
    pub(crate) fn warn(&self, w: Warning) {
        self.warnings.borrow_mut().push(w);
    }

    /// Drain recorded warnings.
    pub(crate) fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut *self.warnings.borrow_mut())
    }
}

#[cfg(test)]
mod tests;
