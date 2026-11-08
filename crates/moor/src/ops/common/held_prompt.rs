//! Gives the agent the prompt a start that had to queue asked of it, exactly once.
//!
//! A relaunched agent is not told at once: right after the launch no hook of its new session has
//! been read, so one slow probe refuses the steer, and the hook file may still hold the old
//! session's last report. The status sweep gives it once the new session reports Idle, and tries
//! again on a later sweep when that steer fails.

use moor_core::activity::AgentState;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::Reading;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::start::AfterWake;
use crate::store::{BoxRow, Store};

/// Failed tries per agent session before the sweep stops; the next launch tries again.
pub(crate) const MAX_TRIES: u32 = 3;

/// Recent events searched for failed tries.
const TRY_EVENTS: usize = 100;

/// Gives the held prompt to an agent the wake left running. A relaunched agent that started on it
/// drops it; one that resumed its session, or one that fails the steer, waits for the sweep.
pub(crate) fn give_after_wake(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    after: AfterWake,
) -> Result<(), CommandError> {
    if store.held_prompt(row.id)?.is_none() {
        return Ok(());
    }
    let fresh = row.agent_session_id.as_deref().is_none_or(str::is_empty);
    match after {
        AfterWake::Relaunched if fresh => {
            store.set_held_prompt(row.id, None)?;
            Ok(())
        }
        AfterWake::Unknown | AfterWake::Relaunched => Ok(()),
        AfterWake::Intact => give(cx, store, row),
    }
}

/// Gives the held prompt once the agent's new session reports Idle, unless an op holds the Box.
pub(crate) fn give_on_sweep(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    if row.lifecycle != BoxLifecycle::Ready || store.held_prompt(row.id)?.is_none() {
        return Ok(());
    }
    let idle = reading
        .activity
        .as_ref()
        .is_some_and(|a| a.state == AgentState::Idle);
    if !idle || !reported_since_launch(store, row)? || failed_tries(store, row)? >= MAX_TRIES {
        return Ok(());
    }
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "steer",
        std::time::Duration::ZERO,
    ) else {
        return Ok(());
    };
    give(cx, store, row)
}

/// Steers the held prompt in and drops it; a failed steer keeps it and records a
/// `prompt_not_given` event.
fn give(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let Some(text) = store.held_prompt(row.id)? else {
        return Ok(());
    };
    if let Err(e) = crate::ops::steer::tell(cx, store, row, text) {
        let event = ExtraEvent::PromptNotGiven {
            attempt: failed_tries(store, row)? + 1,
            error: e.error.to_string(),
        };
        store.apply(Decision::new(row.id).event(event))?;
        return Ok(());
    }
    store.set_held_prompt(row.id, None)?;
    Ok(())
}

/// Whether the live agent session has reported through its hook since it launched.
fn reported_since_launch(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let Some(session) = store.live_session(row.id)? else {
        return Ok(false);
    };
    Ok(store
        .hook(row.id)?
        .is_some_and(|c| c.record.at >= session.started_at))
}

/// Failed tries since the agent's session last started.
fn failed_tries(store: &Store, row: &BoxRow) -> Result<u32, CommandError> {
    let events = store.events(row.id, TRY_EVENTS)?;
    let since_launch = events
        .iter()
        .rev()
        .take_while(|e| e.kind != "session_started")
        .filter(|e| e.kind == "prompt_not_given")
        .count();
    Ok(u32::try_from(since_launch).unwrap_or(u32::MAX))
}
