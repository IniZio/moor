//! The `turn_done` event: the agent finished the turn a sent inbox row started.

use moor_core::activity::{stalled_for, Activity, AgentState, Source, Status};
use moor_core::decide::turn::{turn_done, SteerMark, TermTurn, TurnFacts, TurnSignal};
use moor_core::domain::event::TurnOutcome;
use moor_core::domain::inbox::{Delivery, InboxId};
use moor_core::seam::clock::Ts;

use super::{PaneLiveness, Reading};
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, InboxRow, Store};

/// Answer each sent inbox row whose turn is over, oldest first: delete it and append its
/// `turn_done` event, and make the Box's archive due.
///
/// A row is answered once, since answering deletes it. It is answered under the Box's turn,
/// before any state step or session end so neither can lose it. While another operation holds
/// the turn nothing is written and the next pass reads the ended turn again; true then, so the
/// caller leaves the session alone until the turn end is on record.
pub(super) fn note_turn_done(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    let mut held = None;
    let open = store.sent(row.id)?;
    let newest = open.last().map(|r| r.id);
    for open in open {
        let Some(outcome) = answer(reading, &open, newest, now) else {
            return Ok(false);
        };
        if held.is_none() {
            held = cx.try_turn(row.id, Cmd::Settle);
        }
        if held.is_none() {
            return Ok(true);
        }
        store.answer(row.id, open.id, outcome)?;
    }
    Ok(false)
}

/// Whether a sent row's turn is still open by `reading`.
pub(crate) fn turn_open(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    let open = store.sent(row.id)?;
    let newest = open.last().map(|r| r.id);
    Ok(open
        .iter()
        .any(|open| answer(reading, open, newest, now).is_none()))
}

/// The answer `reading` gives to sent row `open`, when its turn is over. Only the `newest` row
/// can end without its turn showing: a later text may carry an earlier one into its turn.
fn answer(
    reading: &Reading,
    open: &InboxRow,
    newest: Option<InboxId>,
    now: Ts,
) -> Option<TurnOutcome> {
    let Delivery::Sent { at, mark, .. } = open.delivery else {
        return None;
    };
    let facts = TurnFacts {
        steer: Some(SteerMark {
            id: open.id.get(),
            turn_before: mark.turn_before,
            status_before: mark.status_before,
        }),
        term: reading.term.as_ref().map(|t| TermTurn {
            signal: match t.status {
                Some(Status::Idle | Status::Error { .. }) => TurnSignal::Idle,
                Some(Status::Blocked { .. }) => TurnSignal::Blocked,
                Some(Status::Working { .. }) | None => TurnSignal::Other,
            },
            turn: t.turn,
            status_at: t.status_at.map(Ts::as_millis),
        }),
        exited: reading.pane == PaneLiveness::Exited,
    };
    turn_done(&facts).map(|(_, outcome)| outcome).or_else(|| {
        (newest == Some(open.id))
            .then(|| never_began(reading, Ts::from_millis(at), now))
            .flatten()
    })
}

/// Ends a row whose turn never showed: its text never reached the agent as a prompt. An agent
/// read idle, its status unchanged since before a row sent at `sent` that long ago, is not
/// working on it.
fn never_began(reading: &Reading, sent: Ts, now: Ts) -> Option<TurnOutcome> {
    let read_idle = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Idle,
            source: Source::Status,
        })
    );
    let at = reading
        .term
        .as_ref()
        .and_then(|t| t.status_at)
        .filter(|_| read_idle)?;
    let old = stalled_for(sent, now).is_some();
    let quiet = stalled_for(at, now).is_some();
    (old && quiet).then_some(TurnOutcome::Idle)
}
