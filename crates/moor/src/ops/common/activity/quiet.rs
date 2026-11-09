//! Moving a Box between Working and Idle.

use moor_core::activity::AgentState;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::session_state::SessionInput;

use super::launch::warn_launch_timed_out;
use super::said;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store, StoreError};

use super::Reading;

impl Reading {
    /// Whether the agent could not be read at all, which says nothing of what it is doing.
    pub(crate) fn unread(&self) -> bool {
        self.activity
            .as_ref()
            .is_some_and(|a| a.state == AgentState::Unknown)
    }

    /// Whether the reading shows work under way: an unread agent holds a Working Box up, but
    /// does not pull an Idle one back.
    fn shows_work(&self) -> bool {
        self.busy() && !self.unread()
    }
}

/// Steps the Box between Working and Idle to match `reading`.
pub(super) fn step(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    // A steered turn ends only on its `turn_done`: a hook report older than the steer is no answer.
    let turn_open = store.unanswered_steer(row.id)?.is_some();
    let input = match (row.lifecycle == BoxLifecycle::Ready, row.session.is_quiet()) {
        (true, false) if !reading.busy() && reading.may_quiet && !turn_open => {
            SessionInput::AgentQuiet
        }
        (true, true) if reading.shows_work() => SessionInput::AgentBusy,
        _ => return Ok(()),
    };
    // An op that owns the Box decides where the Box goes; hold its turn across the write.
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
        return Ok(());
    };
    if row.intent.is_some() {
        return Ok(());
    }
    let quiet = input == SessionInput::AgentQuiet;
    if !said::record(store, row, quiet, reading)? {
        return Ok(());
    }
    let mut decision = Decision::new(row.id).session(input.clone());
    let timed_out = reading
        .launch_overdue
        .filter(|_| input == SessionInput::AgentQuiet);
    if let Some(after_secs) = timed_out {
        decision = decision.event(ExtraEvent::LaunchTimedOut { after_secs });
    }
    match store.apply(decision) {
        Ok(_) => {
            if let Some(after_secs) = timed_out {
                warn_launch_timed_out(cx, row, after_secs);
            }
            Ok(())
        }
        Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
