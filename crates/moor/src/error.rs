//! Top-level command error and its single conversion to the response envelope.

use moor_core::{ErrorKind, ErrorView, Response, Warning};

mod close;
mod store;

mod driver;
mod surface;

/// Failure of one command, classified once by whoever produced it.
#[derive(Debug)]
pub struct CommandError {
    /// Failure class; decides the exit class.
    pub kind: ErrorKind,
    /// Underlying error; its display text becomes the envelope message.
    pub error: Box<dyn std::error::Error + Send + Sync>,
    /// Suggested next steps for the user.
    pub hints: Vec<String>,
    /// Extra lines describing where the failure happened.
    pub context: Vec<String>,
}

impl CommandError {
    /// Build an error of `kind` from any error or message.
    pub fn new(
        kind: ErrorKind,
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            kind,
            error: error.into(),
            hints: Vec::new(),
            context: Vec::new(),
        }
    }

    /// Error for a command whose operation has not landed yet.
    #[must_use]
    pub fn not_implemented(cmd: &str) -> Self {
        Self::new(
            ErrorKind::Internal,
            format!("`moor {cmd}` is not implemented yet"),
        )
    }

    /// Append one hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// Convert to the failure envelope; the only such conversion.
    #[must_use]
    pub fn into_response(self, cmd: &str, warnings: Vec<Warning>) -> Response {
        let view = ErrorView {
            kind: self.kind,
            message: self.error.to_string(),
            hints: self.hints,
            context: self.context,
        };
        Response::err(cmd, view).with_warnings(warnings)
    }
}

#[cfg(test)]
mod tests;
