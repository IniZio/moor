//! Auto-sleep: park a Box whose agent has been quiet long enough.
//!
//! Runs only where activity already settles Working and Idle. It never waits for the Box
//! lock and never fails the caller.

use std::time::Duration;

use moor_core::decide::sleep::{sleep_due, SleepFacts, SleepVerdict};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::cache_save::{CacheSave, CacheSaveDenied};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::name::{BoxHandle, BoxId};
use moor_core::seam::driver::{PausePublish, SleepOutcome};
use moor_core::{ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::Reading;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::cache::save_decision;
use crate::ops::common::park::{park_tunnel, restore_tunnel};
use crate::ops::common::suspend_check;
use crate::ops::start::revive_after_wake;
use crate::store::{BoxRow, Store, StoreError};

/// What the last read of the agent said, as auto-sleep needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRead {
    /// Read, and no prompt holds it.
    Plain,
    /// It waits on an unanswered prompt or a first-run screen.
    Blocked,
    /// It could not be read, and may have started a turn since it went Idle.
    Unread,
}

impl AgentRead {
    /// How `reading` counts.
    pub(crate) fn of(reading: &Reading) -> Self {
        if reading.blocked() {
            Self::Blocked
        } else if reading.unread() {
            Self::Unread
        } else {
            Self::Plain
        }
    }
}

/// Sleep the machine of Box `id` when it has been Idle for `box.sleep_after`, Idle for
/// `box.sleep_after_blocked` while its agent is blocked on a prompt, or awake for `box.max_awake`.
/// While its agent is unread, only the awake cap sleeps it.
///
/// A held Box lock, a Box that cannot be slept right now and a step the store refuses leave
/// everything as it was. A Box with a recorded refusal is not asked again until it has left Idle. Any other failure becomes a warning.
pub(crate) fn run(cx: &Cx, store: &Store, id: BoxId, agent: AgentRead) {
    report(cx, try_sleep(cx, store, id, agent));
}

/// Warns of a failure other than a refusal.
fn report(cx: &Cx, slept: Result<(), CommandError>) {
    match slept {
        Err(e) if e.kind != ErrorKind::Refused => cx.warn(Warning {
            code: "auto_sleep_failed".to_owned(),
            message: e.error.to_string(),
        }),
        _ => {}
    }
}

/// Whether `row` has been Idle, or awake, long enough to sleep.
fn due(cx: &Cx, store: &Store, row: &BoxRow, agent: AgentRead) -> Result<bool, CommandError> {
    let config = &cx.config().box_;
    let facts = SleepFacts {
        lifecycle: row.lifecycle,
        session: if agent == AgentRead::Unread {
            SessionState::Working
        } else {
            row.session
        },
        idle_since: row.idle_since,
        now: cx.clock().now(),
        after: config.sleep_after(),
        blocked: agent == AgentRead::Blocked,
        after_blocked: config.sleep_after_blocked(),
        awake_since: store.awake_since(row.id)?,
        max_awake: config.max_awake(),
    };
    Ok(sleep_due(&facts) == SleepVerdict::Due)
}

/// Sleep the machine once the Box lock is free and the Box is still due.
fn try_sleep(cx: &Cx, store: &Store, id: BoxId, agent: AgentRead) -> Result<(), CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    if !due(cx, store, &row, agent)? {
        return Ok(());
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", Duration::ZERO)?;
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    if !due(cx, store, &row, agent)? {
        return Ok(());
    }
    if row.boxed.as_ref().is_some_and(|b| b.sleep_reason.is_some()) {
        return Ok(());
    }
    let handle = sleep_handle(cx, &row)?;
    let settled = if agent == AgentRead::Unread {
        Err(CacheSaveDenied::AgentUnread)
    } else {
        settled(cx, store, &row)?
    };
    if !row.session.is_quiet() {
        // Only the awake cap gets here; the sleep ends the agent's pane, so its Session is quiet.
        match store.apply(Decision::new(id).session(SessionInput::AgentQuiet)) {
            Ok(_) => {}
            Err(StoreError::Illegal(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    let parked = sleep_parked(cx, store, &row, &handle, settled)?;
    let decision = match parked.outcome {
        SleepOutcome::Slept | SleepOutcome::Native => Decision::new(id)
            .cache_save(parked.cache_save)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: true,
                reason: None,
            })
            .box_input(BoxInput::Sleep)
            .set(Fields {
                box_sleep_reason: Field::Clear,
                ..Fields::default()
            }),
        SleepOutcome::CannotSleep(reason) => Decision::new(id)
            .set(Fields {
                box_sleep_reason: Field::Set(reason.clone()),
                ..Fields::default()
            })
            .event(ExtraEvent::StaysUp { reason }),
    };
    match store.apply(decision) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// What asking the Driver to sleep a Box came to.
pub(crate) struct Parked {
    /// What the Driver did.
    pub(crate) outcome: SleepOutcome,
    /// What the sleep decided about the caches, for its `asleep` event.
    pub(crate) cache_save: Option<CacheSave>,
}

/// Park the Proxy tunnel, end the agent's pane, then ask the Driver to sleep the machine.
///
/// The agent's ssh session would otherwise outlive the Box until its keepalive gave up. A wake
/// finds the pane gone and attaches a new one, or relaunches an agent that did not survive. A Box
/// that stays awake gets its tunnel back and its agent back the same way. Both are best effort: the sleep result is what the caller needs
/// to see.
///
/// `settled` comes from [`settled`], read before anything marked the agent quiet for this sleep.
pub(crate) fn sleep_parked(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    handle: &BoxHandle,
    settled: Result<(), CacheSaveDenied>,
) -> Result<Parked, CommandError> {
    let id = row.id;
    let cache_save = save_decision(cx, store, row, settled);
    let publish = if cache_save == Some(CacheSave::Allowed) {
        PausePublish::Allowed
    } else {
        PausePublish::Denied
    };
    store.set_pause_publish(id, publish)?;
    // Parking takes the Proxy away, and the check runs in the Box.
    suspend_check::record(cx, store, id, settled.is_ok());
    park_tunnel(cx, id)?;
    let slept = cx.driver().sleep(handle, publish);
    if !matches!(slept, Ok(SleepOutcome::Slept | SleepOutcome::Native)) {
        let _ = restore_tunnel(cx, id);
        if let Err(e) = revive_after_wake(cx, store, id) {
            cx.warn(Warning {
                code: "agent_relaunch_failed".to_owned(),
                message: e.error.to_string(),
            });
        }
    }
    Ok(Parked {
        outcome: slept?,
        cache_save,
    })
}

/// Ok when `row`'s agent is read and quiet and no run is in flight, so nothing it started writes to
/// the Box; which of those fails otherwise.
pub(crate) fn settled(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
) -> Result<Result<(), CacheSaveDenied>, CommandError> {
    Ok(if !row.session.is_quiet() {
        Err(CacheSaveDenied::AgentNotQuiet)
    } else if store.in_flight_run(row.id)?.is_some() {
        Err(CacheSaveDenied::RunInFlight)
    } else if store.agent_unread(row.id, cx.clock().now())? {
        Err(CacheSaveDenied::AgentUnread)
    } else {
        Ok(())
    })
}

/// A refusal with `why`.
pub(crate) fn refuse(why: String) -> CommandError {
    CommandError::new(ErrorKind::Refused, why)
}

/// The handle of the ready Box when no run is in flight, or a refusal.
pub(crate) fn sleep_handle(cx: &Cx, row: &BoxRow) -> Result<BoxHandle, CommandError> {
    let handle = ready_handle(row)?;
    if cx.store()?.in_flight_run(row.id)?.is_some() {
        return Err(refuse("a run is in flight".to_owned()));
    }
    Ok(handle)
}

/// The handle of the ready Box, or a refusal.
pub(crate) fn ready_handle(row: &BoxRow) -> Result<BoxHandle, CommandError> {
    let b = row
        .boxed
        .as_ref()
        .ok_or_else(|| refuse("the Box has no machine".to_owned()))?;
    match b.state {
        BoxState::Ready => {}
        BoxState::Gone | BoxState::Deleted => return Err(refuse("the Box is gone".to_owned())),
        _ => return Err(refuse("the Box is not ready".to_owned())),
    }
    b.handle
        .clone()
        .ok_or_else(|| refuse("the Box has not been acquired".to_owned()))
}

#[cfg(test)]
mod tests;
