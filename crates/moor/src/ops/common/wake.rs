//! Wake an Asleep Task and Box before an op acts on them.
//!
//! The caller holds the Box lock and passes a row read after taking it.

use std::collections::HashMap;
use std::path::Path;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::view::sleep::SleepValue;
use moor_core::{ErrorKind, TaskReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::park::restore_tunnel;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::view::{hide_unsupported_box_sleep, task_view};
use crate::ops::start::revive_after_wake;
use crate::store::{Store, TaskRow};

/// Wake the Task and its Box when either is Asleep, leaving the Proxy tunnel parked.
///
/// For callers that need the Box awake but not reachable through the Proxy.
/// A Gone Box is never resumed. A Box keeps its working directory across
/// pause and resume, so a usable stored workdir is reused without asking the
/// Driver. Only a Box with no usable stored workdir is looked up and stored
/// with the wake. When the Driver fails to resume the Box or to find its
/// workdir, nothing is applied and the Task and Box stay Asleep, so the next
/// call tries both again.
/// `Ok(true)` when it woke anything. The Proxy tunnel stays parked and no agent
/// is relaunched; close uses this since it needs neither.
pub(crate) fn wake_box_if_asleep(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
) -> Result<bool, CommandError> {
    wake(cx, store, row).map(|w| w.any)
}

/// Like `wake_box_if_asleep`, then restores the tunnel and relaunches a gone agent.
///
/// A Box can cold-boot on resume and lose its agent process. The tunnel comes
/// back first because the agent reaches the model through the Proxy. When the
/// tunnel or the relaunch fails the Box stays awake and the error is returned.
pub(crate) fn wake_and_revive(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    let woke = wake_with_tunnel(cx, store, row)?;
    if woke.box_resumed {
        revive_after_wake(cx, store, row.id)?;
    }
    Ok(woke.any)
}

/// Wake the Task and Box, then restore the tunnel when the Box was resumed.
fn wake_with_tunnel(cx: &Cx, store: &Store, row: &TaskRow) -> Result<Woke, CommandError> {
    let woke = wake(cx, store, row)?;
    if woke.box_resumed {
        // The Box stays awake when the tunnel cannot return; the op fails instead.
        restore_tunnel(cx, store, row.id, false)?;
    }
    Ok(woke)
}

/// What `wake` did.
struct Woke {
    /// The Task or the Box was Asleep and is awake now.
    any: bool,
    /// The Driver resumed the Box.
    box_resumed: bool,
}

/// Wake the Task and Box when Asleep and report what was woken.
fn wake(cx: &Cx, store: &Store, row: &TaskRow) -> Result<Woke, CommandError> {
    let task_asleep = row.state == TaskState::Asleep;
    let box_asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    if !task_asleep && !box_asleep {
        return Ok(Woke {
            any: false,
            box_resumed: false,
        });
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
            if b.workdir
                .as_deref()
                .is_none_or(|w| recorded_workdir(w).is_none())
            {
                let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
                workdir = Some(cx.driver().workdir(handle, worktree)?);
            }
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
    Ok(Woke {
        any: true,
        box_resumed: box_asleep,
    })
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
    let woke = wake_and_revive(cx, store, &reload()?)?;
    let row = reload()?;
    held(&row)?;
    let mut boxed = task_view(&row, cx.config()).r#box;
    hide_unsupported_box_sleep(boxed.as_mut(), cx, &mut HashMap::new());
    Ok(SleepValue {
        task: req.task.as_str().to_owned(),
        r#box: boxed,
        woke,
    })
}

#[cfg(test)]
mod tests;
