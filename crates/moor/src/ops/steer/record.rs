//! The `steered` event a sent steer leaves behind.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::{BoxId, ErrorKind};

use crate::error::CommandError;
use crate::store::{SessionRow, Store};

/// What the `steered` event says about the send beyond the message.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Mark {
    /// The ended-turn count of the agent's terminal the steer's turn must pass.
    pub(super) turn_before: Option<u64>,
    /// Whether the submit key was pressed a second time.
    pub(super) submit_retried: bool,
}

/// Append the `steered` event; its id.
pub(super) fn record(
    store: &Store,
    id: BoxId,
    session: &SessionRow,
    message: &str,
    mark: Mark,
) -> Result<i64, CommandError> {
    let who = if session.session_ref.is_empty() {
        session.pane.as_deref().unwrap_or_default()
    } else {
        &session.session_ref
    };
    let event = ExtraEvent::Steered {
        session: who.to_owned(),
        message: message.to_owned(),
        submit_retried: mark.submit_retried,
        turn_before: mark.turn_before,
    };
    let applied = store.apply(Decision::new(id).event(event))?;
    applied
        .events
        .first()
        .copied()
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, "the steered event was not recorded"))
}
