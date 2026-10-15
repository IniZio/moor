//! Auto-sleep: park the Box of a Task whose agent has been quiet long enough.
//!
//! Runs only where activity already settles Working and Idle. It never waits for the Box
//! lock and never fails the caller.

use std::time::Duration;

use moor_core::decide::sleep::{sleep_due, SleepFacts, SleepVerdict};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::task::TaskInput;
use moor_core::name::{BoxHandle, TaskId};
use moor_core::seam::driver::SleepOutcome;
use moor_core::{ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::store::{Store, StoreError, TaskRow};

/// Sleep the Box of task `id` when its Task has been Idle for `box.sleep_after`.
///
/// A held Box lock, a Box that cannot be slept right now and a step the store refuses leave
/// everything as it was. A Box with a recorded refusal is not asked again until the Task has left Idle. Any other failure becomes a warning.
pub(crate) fn run(cx: &Cx, store: &Store, id: TaskId) {
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

/// Whether the Task's Box lives on a host moor cannot sleep.
fn is_static_host(cx: &Cx, row: &TaskRow) -> bool {
    row.boxed
        .as_ref()
        .is_some_and(|b| matches!(cx.driver().caps(&b.provider), Ok(c) if c.static_host))
}

/// Sleep the Box once the Box lock is free and the Task is still due.
fn try_sleep(cx: &Cx, store: &Store, id: TaskId) -> Result<(), CommandError> {
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
    let outcome = if is_static_host(cx, &row) {
        SleepOutcome::CannotSleep("the Box is a static SSH host; moor never sleeps it".to_owned())
    } else {
        cx.driver().sleep(&handle)?
    };
    let decision = match outcome {
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
