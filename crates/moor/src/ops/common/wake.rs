//! Wake an Asleep Task and Box before an op acts on them.
//!
//! The caller holds the Box lock and passes a row read after taking it.

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::view::sleep::SleepValue;
use moor_core::{ErrorKind, TaskReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::view::task_view;
use crate::store::{Store, TaskRow};

/// Wake the Task and its Box when either is Asleep; `Ok(true)` when it did.
///
/// A Gone Box is never resumed. A resumed Box may sit in a different working
/// directory, so it is looked up again and stored with the wake. When the
/// Driver fails to resume the Box or to find its workdir, nothing is applied
/// and the Task and Box stay Asleep, so the next call tries both again.
pub(crate) fn wake_if_asleep(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    let task_asleep = row.state == TaskState::Asleep;
    let box_asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    if !task_asleep && !box_asleep {
        return Ok(false);
    }
    let mut workdir = None;
    if let Some(b) = &row.boxed {
        if matches!(b.state, BoxState::Gone | BoxState::Deleted) {
            return Err(CommandError::new(ErrorKind::Refused, "the Box is gone"));
        }
        if box_asleep {
            let handle = b.handle.as_ref().ok_or_else(|| {
                CommandError::new(ErrorKind::Refused, "the Box has not been acquired")
            })?;
            cx.driver().wake(handle)?;
            workdir = Some(cx.driver().workdir(handle)?);
        }
    }
    let mut d = Decision::new(row.id);
    if task_asleep {
        d = d.task_input(TaskInput::Wake);
    }
    if box_asleep {
        d = d.box_input(BoxInput::Wake);
    }
    if let Some(workdir) = workdir {
        d = d.set(Fields {
            box_workdir: Field::Set(workdir.as_str().to_owned()),
            ..Fields::default()
        });
    }
    store.apply(d)?;
    Ok(true)
}

/// Wake the open Task named by `req` under its Box lock and build the value.
///
/// `held` runs on the woken row while the lock is still held.
pub(crate) fn wake_task(
    cx: &Cx,
    req: &TaskReq,
    op: &str,
    held: impl FnOnce(&TaskRow) -> Result<(), CommandError>,
) -> Result<SleepValue, CommandError> {
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
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, op, boxlock::WAIT)?;
    let reload = || {
        store.task(id)?.ok_or_else(|| {
            CommandError::new(ErrorKind::Failed, format!("no task {}", req.task.as_str()))
        })
    };
    let woke = wake_if_asleep(cx, store, &reload()?)?;
    let row = reload()?;
    held(&row)?;
    Ok(SleepValue {
        task: req.task.as_str().to_owned(),
        r#box: task_view(&row, cx.config()).r#box,
        woke,
    })
}

#[cfg(test)]
mod tests;
