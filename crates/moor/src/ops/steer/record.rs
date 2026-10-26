//! The `steered` event a sent steer leaves behind.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, ErrorKind};

use crate::error::CommandError;
use crate::store::{SessionRow, Store};

/// What the `steered` event says about the send beyond the message.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Mark {
    /// When the newest hook report read before the send was written, on the Box's clock.
    pub(super) hook_before: Option<Ts>,
    /// Whether the steer typed a prompt that starts a turn of its own.
    pub(super) new_turn: bool,
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
        hook_before: mark.hook_before.map(Ts::as_millis),
        new_turn: mark.new_turn,
    };
    let applied = store.apply(Decision::new(id).event(event))?;
    applied
        .events
        .first()
        .copied()
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, "the steered event was not recorded"))
}
