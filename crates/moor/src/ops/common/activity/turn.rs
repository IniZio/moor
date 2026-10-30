//! The `turn_done` event: the agent finished the turn a steer started.

use moor_core::activity::HookKind;
use moor_core::decide::turn::{turn_done, HookFacts, HookSignal, TurnFacts};

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
    let Some((steer, outcome)) = turn_done(&facts) else {
        return Ok(false);
    };
    store.record_turn_done(row.id, steer, outcome)?;
    Ok(true)
}
