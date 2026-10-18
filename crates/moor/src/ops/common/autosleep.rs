//! Auto-sleep: park the Box of a Task whose agent has been quiet long enough.
//!
//! Runs only where activity already settles Working and Idle. It never waits for the Box
//! lock and never fails the caller.

use std::time::Duration;

use moor_core::decide::sleep::{sleep_due, SleepFacts, SleepVerdict};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::task::TaskInput;
use moor_core::name::{BoxHandle, BoxId};
use moor_core::seam::driver::SleepOutcome;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::{ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::park::{park_tunnel, restore_tunnel};
use crate::ops::start::revive_after_wake;
use crate::store::{Store, StoreError, TaskRow};

/// Sleep the Box of task `id` when its Task has been Idle for `box.sleep_after`.
///
/// A held Box lock, a Box that cannot be slept right now and a step the store refuses leave
/// everything as it was. A Box with a recorded refusal is not asked again until the Task has left Idle. Any other failure becomes a warning.
pub(crate) fn run(cx: &Cx, store: &Store, id: BoxId) {
    match try_sleep(cx, store, id) {
        Err(e) if e.kind != ErrorKind::Refused => cx.warn(Warning {
            code: "auto_sleep_failed".to_owned(),
            message: e.error.to_string(),
        }),
        _ => {}
    }
}

/// Whether `row` has been Idle long enough to sleep.
fn due(cx: &Cx, row: &TaskRow) -> bool {
    let facts = SleepFacts {
        state: row.state,
        idle_since: row.idle_since,
        now: cx.clock().now(),
        after: cx.config().box_.sleep_after(),
    };
    sleep_due(&facts) == SleepVerdict::Due
}

/// Sleep the Box once the Box lock is free and the Task is still due.
fn try_sleep(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let Some(row) = store.task(id)? else {
        return Ok(());
    };
    if !due(cx, &row) {
        return Ok(());
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", Duration::ZERO)?;
    let Some(row) = store.task(id)? else {
        return Ok(());
    };
    if !due(cx, &row) {
        return Ok(());
    }
    if row.boxed.as_ref().is_some_and(|b| b.sleep_reason.is_some()) {
        return Ok(());
    }
    let handle = sleep_handle(cx, &row)?;
    let decision = match sleep_parked(cx, store, id, &handle)? {
        SleepOutcome::Slept | SleepOutcome::Native => Decision::new(id)
            .task_input(TaskInput::SleepDue)
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

/// Park the Task's Proxy tunnel, end the agent's pane, then ask the Driver to sleep the Box.
///
/// The agent's ssh session would otherwise outlive the Box until its keepalive gave up. A wake
/// finds the pane gone and relaunches the agent. A Box that stays awake gets its tunnel back
/// and its agent relaunched. Both are best effort: the sleep result is what the caller needs
/// to see.
pub(crate) fn sleep_parked(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    handle: &BoxHandle,
) -> Result<SleepOutcome, CommandError> {
    park_tunnel(cx, store, id)?;
    if let Err(e) = close_agent_pane(cx, store, id) {
        let _ = restore_tunnel(cx, store, id, true);
        return Err(e);
    }
    let slept = cx.driver().sleep(handle);
    if !matches!(slept, Ok(SleepOutcome::Slept | SleepOutcome::Native)) {
        let _ = restore_tunnel(cx, store, id, true);
        if let Err(e) = revive_after_wake(cx, store, id) {
            cx.warn(Warning {
                code: "agent_relaunch_failed".to_owned(),
                message: e.error.to_string(),
            });
        }
    }
    Ok(slept?)
}

/// Closes the live agent pane; a pane already gone is fine.
fn close_agent_pane(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let Some(session) = store.live_session(id)? else {
        return Ok(());
    };
    let Some(pane) = session.pane.as_deref() else {
        return Ok(());
    };
    match cx.surface().close_pane(&PaneRef::new(pane)) {
        Ok(()) | Err(SurfaceError::NotFound) => Ok(()),
        Err(e) => Err(CommandError::new(
            ErrorKind::Failed,
            format!("could not close the agent pane: {e}; the Box stays awake"),
        )),
    }
}

/// A refusal with `why`.
pub(crate) fn refuse(why: String) -> CommandError {
    CommandError::new(ErrorKind::Refused, why)
}

/// The handle of the Task's ready Box when no run is in flight, or a refusal.
pub(crate) fn sleep_handle(cx: &Cx, row: &TaskRow) -> Result<BoxHandle, CommandError> {
    let b = row
        .boxed
        .as_ref()
        .ok_or_else(|| refuse("the task has no Box".to_owned()))?;
    match b.state {
        BoxState::Ready => {}
        BoxState::Gone | BoxState::Deleted => return Err(refuse("the Box is gone".to_owned())),
        _ => return Err(refuse("the Box is not ready".to_owned())),
    }
    let handle = b
        .handle
        .clone()
        .ok_or_else(|| refuse("the Box has not been acquired".to_owned()))?;
    if cx
        .store()?
        .in_flight_run(row.id, &|pid| cx.proc().alive(pid))?
        .is_some()
    {
        return Err(refuse("a run is in flight".to_owned()));
    }
    Ok(handle)
}

#[cfg(test)]
mod tests;
