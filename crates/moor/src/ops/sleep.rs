//! `sleep`: park a Task's Box.
//!
//! Holds the Box lock for the call. Works from Idle, or from Working when no run is in flight.

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::name::BoxHandle;
use moor_core::seam::driver::SleepOutcome;
use moor_core::view::sleep::SleepValue;
use moor_core::{ErrorKind, TaskReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::view::task_view;
use crate::store::TaskRow;

/// Run `sleep`.
pub(crate) fn run(cx: &Cx, req: &TaskReq) -> Result<SleepValue, CommandError> {
    let store = cx.store()?;
    let id = store
        .open_task_by_name(&req.task)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no open task {}", req.task.as_str()),
            )
        })?
        .id;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", boxlock::WAIT)?;
    let reload = || {
        store.task(id)?.ok_or_else(|| {
            CommandError::new(ErrorKind::Failed, format!("no task {}", req.task.as_str()))
        })
    };
    let row = reload()?;
    if row.state != TaskState::Asleep {
        sleep_box(cx, &row)?;
    }
    Ok(SleepValue {
        task: req.task.as_str().to_owned(),
        r#box: task_view(&reload()?).r#box,
        woke: false,
    })
}

/// Ask the Driver to sleep the Box and record the outcome.
fn sleep_box(cx: &Cx, row: &TaskRow) -> Result<(), CommandError> {
    let store = cx.store()?;
    if !matches!(row.state, TaskState::Idle | TaskState::Working) {
        return Err(refuse(format!("the task is {}", row.state.as_str())));
    }
    let handle = usable_handle(row)?;
    if store
        .in_flight_run(row.id, &|pid| cx.proc().alive(pid))?
        .is_some()
    {
        return Err(refuse("a run is in flight".to_owned()));
    }
    match cx.driver().sleep(&handle)? {
        SleepOutcome::Slept | SleepOutcome::Native => {
            let fields = Fields {
                box_sleep_reason: Field::Clear,
                ..Fields::default()
            };
            store.apply(
                Decision::new(row.id)
                    .task_input(TaskInput::SleepCmd)
                    .box_input(BoxInput::Sleep)
                    .set(fields),
            )?;
            Ok(())
        }
        SleepOutcome::CannotSleep(reason) => {
            let fields = Fields {
                box_sleep_reason: Field::Set(reason.clone()),
                ..Fields::default()
            };
            store.apply(Decision::new(row.id).set(fields))?;
            Err(refuse(format!("the Box cannot sleep: {reason}")))
        }
    }
}

/// A refusal with `why`.
fn refuse(why: String) -> CommandError {
    CommandError::new(ErrorKind::Refused, why)
}

/// The handle of the Task's ready Box, or a refusal.
fn usable_handle(row: &TaskRow) -> Result<BoxHandle, CommandError> {
    let b = row
        .boxed
        .as_ref()
        .ok_or_else(|| refuse("the task has no Box".to_owned()))?;
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
