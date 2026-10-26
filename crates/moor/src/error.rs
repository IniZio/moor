//! Top-level command error and its single conversion to the response envelope.

use moor_core::{ErrorDetail, ErrorKind, ErrorView, Response, Warning};

mod close;
mod config;
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
    /// Structured detail a client can act on without reading the message.
    pub detail: Option<Box<ErrorDetail>>,
    /// Outcome still reported in the envelope `value`, when the command made one.
    pub value: Option<Box<serde_json::Value>>,
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
            detail: None,
            value: None,
        }
    }

    /// Append one hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// Attach the structured detail a client reads instead of the message.
    #[must_use]
    pub fn with_detail(mut self, detail: ErrorDetail) -> Self {
        self.detail = Some(Box::new(detail));
        self
    }

    /// Attach the outcome the envelope `value` keeps despite the failure.
    #[must_use]
    pub fn with_value(mut self, value: serde_json::Value) -> Self {
        self.value = Some(Box::new(value));
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
            detail: self.detail.map(|d| *d),
        };
        let mut resp = Response::err(cmd, view).with_warnings(warnings);
        resp.value = self.value.map(|v| *v);
        resp
    }
}

#[cfg(test)]
mod tests;
