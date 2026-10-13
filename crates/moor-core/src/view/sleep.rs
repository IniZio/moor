//! Typed `value` payload for `sleep` and `wake` (JSON wire shape).

use super::BoxView;

/// Result of `sleep` or `wake`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SleepValue {
    /// Task name.
    pub task: String,
    /// The Task's Box after the command.
    #[serde(rename = "box")]
    pub r#box: Option<BoxView>,
    /// Whether this call resumed the Box; `sleep` always reports false.
    pub woke: bool,
}
