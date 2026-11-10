//! The `turn_done` event: the agent finished the turn a sent inbox row started.

use moor_core::activity::{stalled_for, Activity, AgentState, Source, Status, STARTUP_GRACE};
use moor_core::decide::turn::{turn_done, SteerMark, TermTurn, TurnFacts, TurnSignal};
use moor_core::domain::event::TurnOutcome;
use moor_core::domain::inbox::{Delivery, SendMark};
use moor_core::seam::clock::Ts;

use super::{PaneLiveness, Reading};
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, InboxRow, Store};

/// Answer the newest sent inbox row once its turn is over: delete it, and every older sent row
/// that went into the same prompt, append its `turn_done` event, and make the Box's archive due.
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
    let Some(open) = store.sent(row.id)?.pop() else {
        return Ok(false);
    };
    let Some(outcome) = answer(reading, &open, now) else {
        return Ok(false);
    };
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
        return Ok(true);
    };
    store.answer(row.id, open.id, outcome)?;
    Ok(false)
}

/// Whether the newest sent row's turn is still open by `reading`.
pub(crate) fn turn_open(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    Ok(store
        .sent(row.id)?
        .last()
        .is_some_and(|open| answer(reading, open, now).is_none()))
}

/// The answer `reading` gives to sent row `open`, when its turn is over.
fn answer(reading: &Reading, open: &InboxRow, now: Ts) -> Option<TurnOutcome> {
    let Delivery::Sent { launch, at, mark } = open.delivery else {
        return None;
    };
    match reading.launch {
        Some(read) if read < launch => return None,
        Some(read) if read > launch => return Some(TurnOutcome::Exited),
        _ => {}
    }
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
    turn_done(&facts)
        .map(|(_, outcome)| outcome)
        .or_else(|| never_taken(reading, mark, Ts::from_millis(at), now))
        .or_else(|| never_began(reading, Ts::from_millis(at), now))
}

/// Ends a first prompt the agent never took up: sent before any turn ended, with the agent still
/// idle at turn 0 once [`STARTUP_GRACE`] has passed since `sent`, as its activity then reads.
fn never_taken(reading: &Reading, mark: SendMark, sent: Ts, now: Ts) -> Option<TurnOutcome> {
    let first = mark.turn_before == Some(0) && mark.status_before.is_none();
    let idle = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Idle,
            source: Source::Status,
        })
    );
    let fresh = reading.term.as_ref().is_some_and(|t| t.turn == 0);
    let past = now.as_millis() >= sent.saturating_add(STARTUP_GRACE).as_millis();
    (first && idle && fresh && past).then_some(TurnOutcome::Idle)
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
