//! Typed `value` payload for `close-failed` (JSON wire shape).

use crate::name::BoxName;

/// Result of `close-failed`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CloseFailedView {
    /// The Boxes this call closed.
    pub closed: Vec<BoxName>,
    /// The Boxes it could not close, each with why.
    pub failed: Vec<CloseFailedError>,
}

/// One Box that `close-failed` could not close.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CloseFailedError {
    /// The Box.
    #[serde(rename = "box")]
    pub r#box: BoxName,
    /// Why it stayed open.
    pub error: String,
}
