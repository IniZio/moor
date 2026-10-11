//! Per-invocation context handed to operations.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use moor_core::Warning;

/// State shared by one command run.
pub(crate) struct Cx {
    /// Resolved state directory.
    state_dir: Option<PathBuf>,
    /// Warnings recorded so far.
    warnings: RefCell<Vec<Warning>>,
}

impl Cx {
    /// Create a context with an optional resolved state directory.
    pub(crate) const fn new(state_dir: Option<PathBuf>) -> Self {
        Self {
            state_dir,
            warnings: RefCell::new(Vec::new()),
        }
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
