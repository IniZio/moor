//! Typed `value` payload for `sleep` and `wake` (JSON wire shape).

use super::DriverView;

/// Result of `sleep` or `wake`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SleepValue {
    /// Task name.
    pub task: String,
    /// The Task's Box after the command.
    pub driver: Option<DriverView>,
    /// Whether this call resumed the Box; `sleep` always reports false.
    pub woke: bool,
}
