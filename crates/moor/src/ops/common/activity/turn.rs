//! The `turn_done` event: the agent finished the turn a steer started.

use moor_core::activity::{stalled_for, Activity, AgentState, HookKind, Source};
use moor_core::decide::turn::{turn_done, HookFacts, HookSignal, SteerMark, TurnFacts};
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
    let cache = store.hook(row.id)?;
    let facts = TurnFacts {
        steer: Some(steer),
        hook: cache.as_ref().map(|c| HookFacts {
            signal: match c.record.kind {
                HookKind::Idle => HookSignal::Idle,
                HookKind::Blocked => HookSignal::Blocked,
                HookKind::Starting { .. } | HookKind::Busy | HookKind::Waiting { .. } => {
                    HookSignal::Other
                }
            },
            at: c.record.at,
            turn: c.record.turn,
        }),
        pane_exited: reading.pane == PaneLiveness::Exited,
    };
    let done = turn_done(&facts);
    let Some((steer, outcome)) = done.or(never_began(store, row, reading, steer, now)?) else {
        return Ok(false);
    };
    store.record_turn_done(row.id, steer, outcome)?;
    Ok(true)
}

/// Ends a steer whose turn never showed: its text never reached the agent as a prompt, or that
/// prompt ended with no report. A prompt that took writes a busy report at once, so an agent freshly
/// read as idle, with no report since before a steer that old, is not working on it.
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
            source: Source::Hooks,
        })
    );
    let Some(cache) = store.hook(row.id)?.filter(|_| read_idle) else {
        return Ok(None);
    };
    let sent = store.newest_event_of(row.id, "steered")?;
    let old = sent.is_some_and(|ev| stalled_for(ev.ts, now).is_some());
    let quiet = stalled_for(cache.record.at, now).is_some();
    Ok((old && quiet).then_some((steer.id, TurnOutcome::Idle)))
}
