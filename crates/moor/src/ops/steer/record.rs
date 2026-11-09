//! The `steered` event a sent steer leaves behind, and where the agent's terminal was at it.

use moor_core::activity::{Status, Term};
use moor_core::domain::event::{Event, SteerSent};
use moor_core::domain::inbox::{InboxId, SendMark};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::Steer;
use moor_core::view::steer::{Delivery, SteerValue};
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::{Launch, Store};

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

/// Append the `steered` event and confirm inbox row `inbox`, which the steer was sent from, in
/// the same transaction; the event's id.
pub(super) fn record(
    store: &Store,
    id: BoxId,
    session: &Launch,
    message: &str,
    mark: Mark,
    inbox: InboxId,
) -> Result<i64, CommandError> {
    let event = Event::Steered(SteerSent {
        session: session.session_ref.clone(),
        message: message.to_owned(),
        submit_retried: mark.submit_retried,
        turn_before: mark.turn_before,
        status_before: mark.status_before,
        inbox: Some(inbox.get()),
    });
    let sent = SendMark {
        turn_before: mark.turn_before,
        status_before: mark.status_before,
    };
    Ok(store.confirm_sent(id, inbox, sent, &event)?.1)
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

/// The value of a steer the agent took from row `inbox`, recorded as `steered` event `steer`.
pub(super) fn delivered(
    name: &str,
    sent: String,
    answered_menu: bool,
    (steer, inbox): (i64, InboxId),
) -> SteerValue {
    SteerValue {
        box_name: name.to_owned(),
        sent,
        answered_menu,
        steer: Some(steer),
        delivery: Delivery::Delivered,
        inbox: inbox.get(),
    }
}
