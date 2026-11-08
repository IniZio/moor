//! The `turn_done` event: the agent finished the turn a steer started.

use moor_core::activity::{stalled_for, Activity, AgentState, Source, Status};
use moor_core::decide::turn::{turn_done, SteerMark, TermTurn, TurnFacts, TurnSignal};
use moor_core::domain::event::TurnOutcome;
use moor_core::seam::clock::Ts;

use super::{PaneLiveness, Reading};
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Append one `turn_done` event when the newest unanswered steer's turn is over.
///
/// A steer that already has a `turn_done` is not answered again. It is written before any
/// state step or session end so neither can lose it. Returns whether one was written.
pub(super) fn note_turn_done(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    let Some(steer) = store.unanswered_steer(row.id)? else {
        return Ok(false);
    };
    let facts = TurnFacts {
        steer: Some(steer),
        term: reading.term.as_ref().map(|t| TermTurn {
            signal: match t.status {
                Some(Status::Idle | Status::Error { .. }) => TurnSignal::Idle,
                Some(Status::Blocked { .. }) => TurnSignal::Blocked,
                Some(Status::Working { .. }) | None => TurnSignal::Other,
            },
            turn: t.turn,
        }),
        exited: reading.pane == PaneLiveness::Exited,
    };
    let done = turn_done(&facts);
    let Some((steer, outcome)) = done.or(never_began(store, row, reading, steer, now)?) else {
        return Ok(false);
    };
    store.record_turn_done(row.id, steer, outcome)?;
    Ok(true)
}

/// Ends a steer whose turn never showed: its text never reached the agent as a prompt. An agent
/// read idle, its status unchanged since before a steer that old, is not working on it.
fn never_began(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    steer: SteerMark,
    now: Ts,
) -> Result<Option<(i64, TurnOutcome)>, CommandError> {
    let read_idle = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Idle,
            source: Source::Status,
        })
    );
    let Some(at) = reading
        .term
        .as_ref()
        .and_then(|t| t.status_at)
        .filter(|_| read_idle)
    else {
        return Ok(None);
    };
    let Some(sent) = store.newest_event_of(row.id, "steered")? else {
        return Ok(None);
    };
    let old = stalled_for(sent.ts, now).is_some();
    let quiet = stalled_for(at, now).is_some();
    Ok((old && quiet).then_some((steer.id, TurnOutcome::Idle)))
}
