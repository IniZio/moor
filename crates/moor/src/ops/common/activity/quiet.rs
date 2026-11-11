//! Moving a Box between Working and Idle.

use moor_core::activity::AgentState;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::ExtraEvent;
use moor_core::domain::session_state::SessionInput;

use super::launch::warn_launch_timed_out;
use super::said;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::stale::unless_stale;
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
    // A row sent to an agent other than the one read is no turn of it.
    let turn_open = store
        .sent(row.id)?
        .iter()
        .any(|r| r.delivery.launch() == reading.launch);
    let ready = row.lifecycle == BoxLifecycle::Ready;
    let input = match (ready, row.session.is_quiet()) {
        (true, false) if !reading.busy() && reading.may_quiet && !turn_open => {
            Some(SessionInput::AgentQuiet)
        }
        (true, true) if reading.shows_work() => Some(SessionInput::AgentBusy),
        _ => None,
    };
    let says = ready && !turn_open && reading.said.is_some();
    if input.is_none() && !says {
        return Ok(());
    }
    // An op that owns the Box decides where the Box goes; hold its turn across the write.
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
        return Ok(());
    };
    if row.effect.is_some() {
        return Ok(());
    }
    let reply = if says {
        said::reply(store, reading)?
    } else {
        None
    };
    if let Some(text) = reply {
        let said = reading
            .decision(row.id)
            .event(ExtraEvent::AgentSaid { text });
        if unless_stale(cx, store.apply(said))?.is_none() {
            return Ok(());
        }
    }
    let Some(input) = input else {
        return Ok(());
    };
    let quiet = input == SessionInput::AgentQuiet;
    let mut decision = reading.decision(row.id).session(input);
    let timed_out = reading.launch_overdue.filter(|_| quiet);
    if let Some(after_secs) = timed_out {
        decision = decision.event(ExtraEvent::LaunchTimedOut { after_secs });
    }
    match unless_stale(cx, store.apply(decision)) {
        Ok(Some(_)) => {
            if let Some(after_secs) = timed_out {
                warn_launch_timed_out(cx, row, after_secs);
            }
            Ok(())
        }
        Ok(None) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
