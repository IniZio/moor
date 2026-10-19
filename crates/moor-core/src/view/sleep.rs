//! Typed `value` payload for `sleep` and `wake` (JSON wire shape).

use super::DriverView;

/// Result of `sleep` or `wake`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SleepValue {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: String,
    /// The Box after the command.
    pub driver: Option<DriverView>,
    /// Whether this call resumed the Box; `sleep` always reports false.
    pub woke: bool,
}
