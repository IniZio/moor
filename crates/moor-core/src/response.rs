//! Response envelope shared by every command.

use serde::{Deserialize, Serialize};

/// Envelope schema identifier.
pub const SCHEMA: &str = "moor/1";

/// Outcome class of a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitClass {
    /// Success.
    Ok,
    /// Command failed.
    Failed,
    /// Bad usage.
    Usage,
    /// Refused by a guard.
    Refused,
}

/// Kind of an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    /// Bad usage.
    Usage,
    /// Refused by a guard.
    Refused,
    /// Command failed.
    Failed,
    /// Bug in moor.
    Internal,
}

impl From<ErrorKind> for ExitClass {
    /// Map an error kind to its exit class.
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::Usage => Self::Usage,
            ErrorKind::Refused => Self::Refused,
            ErrorKind::Failed | ErrorKind::Internal => Self::Failed,
        }
    }
}

/// Non-fatal notice attached to a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// Stable machine code.
    pub code: String,
    /// Human message.
    pub message: String,
}

/// Error detail of a failed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorView {
    /// Error kind.
    pub kind: ErrorKind,
    /// Human message.
    pub message: String,
    /// Suggested next steps.
    pub hints: Vec<String>,
    /// Context lines.
    pub context: Vec<String>,
}

/// The envelope every command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Response {
    /// Schema identifier.
    pub schema: &'static str,
    /// Command name.
    pub cmd: String,
    /// Outcome class.
    pub class: ExitClass,
    /// Command result.
    pub value: Option<serde_json::Value>,
    /// Non-fatal notices.
    pub warnings: Vec<Warning>,
    /// Error detail when not ok.
    pub error: Option<ErrorView>,
}

/// Wire form of [`Response`]; the schema is checked on conversion so the
/// public field can stay `&'static str`.
#[derive(Deserialize)]
#[allow(
    clippy::missing_docs_in_private_items,
    reason = "fields mirror Response one to one"
)]
struct ResponseWire {
    schema: String,
    cmd: String,
    class: ExitClass,
    value: Option<serde_json::Value>,
    warnings: Vec<Warning>,
    error: Option<ErrorView>,
}

impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::try_from(ResponseWire::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<ResponseWire> for Response {
    type Error = String;

    fn try_from(w: ResponseWire) -> Result<Self, String> {
        if w.schema != SCHEMA {
            return Err(format!(
                "unsupported schema `{}`, expected `{SCHEMA}`",
                w.schema
            ));
        }
        Ok(Self {
            schema: SCHEMA,
            cmd: w.cmd,
            class: w.class,
            value: w.value,
            warnings: w.warnings,
            error: w.error,
        })
    }
}

impl Response {
    /// Build a successful response.
    pub fn ok(cmd: impl Into<String>, value: Option<serde_json::Value>) -> Self {
        Self {
            schema: SCHEMA,
            cmd: cmd.into(),
            class: ExitClass::Ok,
            value,
            warnings: Vec::new(),
            error: None,
        }
    }

    /// Build a failed response; class follows the error kind.
    pub fn err(cmd: impl Into<String>, error: ErrorView) -> Self {
        Self {
            schema: SCHEMA,
            cmd: cmd.into(),
            class: error.kind.into(),
            value: None,
            warnings: Vec::new(),
            error: Some(error),
        }
    }

    /// Attach warnings.
    #[must_use]
    pub fn with_warnings(mut self, warnings: Vec<Warning>) -> Self {
        self.warnings = warnings;
        self
    }
}

#[cfg(test)]
mod tests;
