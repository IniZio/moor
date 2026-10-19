//! The `turn_done` event: the agent finished the turn a steer started.

use moor_core::decide::turn::{turn_done, TurnFacts};
use moor_core::domain::decision::{Decision, ExtraEvent};

use super::{PaneLiveness, Reading};
use crate::error::CommandError;
use crate::store::{Store, TaskRow};

/// Append one `turn_done` event when the newest unanswered steer's turn is over.
///
/// A steer that already has a `turn_done` is not answered again. It is written before any
/// state step or session end so neither can lose it.
pub(super) fn note_turn_done(
    store: &Store,
    row: &TaskRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    let Some(steer) = store.unanswered_steer(row.id)? else {
        return Ok(());
    };
    let cache = store.hook(row.id).ok().flatten();
    let facts = TurnFacts {
        steer: Some(steer),
        hook: cache.as_ref().map(|c| &c.record),
        pane_exited: reading.pane == PaneLiveness::Exited,
    };
    if let Some((steer, outcome)) = turn_done(&facts) {
        store.apply(Decision::new(row.id).event(ExtraEvent::TurnDone { steer, outcome }))?;
    }
    Ok(())
}
