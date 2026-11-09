//! The `steered` event a sent steer leaves behind, and where the agent's terminal was at it.

use moor_core::activity::{Status, Term};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::{Event, SteerSent};
use moor_core::domain::inbox::{InboxId, SendMark};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::Steer;
use moor_core::{BoxId, ErrorKind};

use crate::error::CommandError;
use crate::store::{SessionRow, Store};

/// What the `steered` event says about the send beyond the message.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Mark {
    /// The ended-turn count of the agent's terminal the steer's turn must pass.
    pub(super) turn_before: Option<u64>,
    /// When the status the agent showed was set, when no turn was running.
    pub(super) status_before: Option<i64>,
    /// Whether the submit key was pressed a second time.
    pub(super) submit_retried: bool,
}

/// Append the `steered` event, confirming inbox row `inbox` in the same transaction when the
/// steer was sent from one; its id.
pub(super) fn record(
    store: &Store,
    id: BoxId,
    session: &SessionRow,
    message: &str,
    mark: Mark,
    inbox: Option<InboxId>,
) -> Result<i64, CommandError> {
    let event = ExtraEvent::Steered(SteerSent {
        session: session.session_ref.clone(),
        message: message.to_owned(),
        submit_retried: mark.submit_retried,
        turn_before: mark.turn_before,
        status_before: mark.status_before,
        inbox: inbox.map(InboxId::get),
    });
    if let Some(row) = inbox {
        let sent = SendMark {
            turn_before: mark.turn_before,
            status_before: mark.status_before,
        };
        return Ok(store.confirm_sent(id, row, sent, &Event::from(event))?.1);
    }
    let applied = store.apply(Decision::new(id).event(event))?;
    applied
        .events
        .first()
        .copied()
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, "the steered event was not recorded"))
}

/// The ended-turn count the turn of `steer` must pass: a prompt typed while the agent works waits
/// for the turn already running to end first.
pub(super) fn turn_before(term: Option<&Term>, steer: &Steer) -> Option<u64> {
    let term = term?;
    let working = matches!(term.status, Some(Status::Working { .. }));
    let queued = working && matches!(steer, Steer::Text(_));
    Some(term.turn + u64::from(queued))
}

/// When the status of an agent not working was set: any later status that is not working ends
/// the steer's turn, even one that ends no turn, such as an interrupted question.
pub(super) fn status_before(term: Option<&Term>) -> Option<i64> {
    let term = term.filter(|t| !matches!(t.status, Some(Status::Working { .. })))?;
    term.status_at.map(Ts::as_millis)
}

/// Whether the agent's terminal reports no turn running, so an interrupt has nothing to stop.
pub(super) fn idle(term: Option<&Term>) -> bool {
    term.is_some_and(|t| matches!(t.status, Some(Status::Idle | Status::Error { .. })))
}
