//! Typed `value` payload for `sleep` and `wake` (JSON wire shape).

use super::DriverView;
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;

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

/// Whether a Box is asleep with its agent mid-turn.
///
/// Sleep keeps the Session state, so a busy Session on an asleep Box means the turn did not finish.
#[must_use]
pub const fn unfinished_turn(lifecycle: BoxLifecycle, session: SessionState) -> bool {
    matches!(lifecycle, BoxLifecycle::Asleep) && !session.is_quiet()
}
