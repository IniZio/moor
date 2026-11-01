//! Moving a Box between Working and Idle.

use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::session_state::SessionInput;

use super::launch::warn_launch_timed_out;
use super::said;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::store::{BoxRow, Store, StoreError};

use super::Reading;

/// Steps the Box between Working and Idle to match `reading`; true when it went Idle.
pub(super) fn step(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<bool, CommandError> {
    // A steered turn ends only on its `turn_done`: a hook report older than the steer is no answer.
    let turn_open = store.unanswered_steer(row.id)?.is_some();
    let input = match (row.lifecycle == BoxLifecycle::Ready, row.session.is_quiet()) {
        (true, false) if !reading.busy() && reading.may_quiet && !turn_open => {
            SessionInput::AgentQuiet
        }
        (true, true) if reading.busy() => SessionInput::AgentBusy,
        _ => return Ok(false),
    };
    // An op that owns the Box decides where the Box goes; hold the lock across the write.
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "settle",
        Duration::ZERO,
    ) else {
        return Ok(false);
    };
    if row.intent.is_some() {
        return Ok(false);
    }
    let quiet = input == SessionInput::AgentQuiet;
    if !said::record(store, row, quiet, reading.said.as_deref())? {
        return Ok(false);
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
            Ok(input == SessionInput::AgentQuiet)
        }
        Err(StoreError::Illegal(_)) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
