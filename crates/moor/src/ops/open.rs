//! `open`: wake a Task's asleep Box, then focus its Workspace.
//!
//! Holds the Box lock while the Box resumes.

use moor_core::seam::surface::WorkspaceRef;
use moor_core::view::sleep::SleepValue;
use moor_core::{ErrorKind, TaskReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::view::task_view;
use crate::ops::common::wake::wake_if_asleep;

/// Run `open`.
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
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "open", boxlock::WAIT)?;
    let reload = || {
        store.task(id)?.ok_or_else(|| {
            CommandError::new(ErrorKind::Failed, format!("no task {}", req.task.as_str()))
        })
    };
    let woke = wake_if_asleep(cx, store, &reload()?)?;
    let row = reload()?;
    let workspace = row
        .workspace_id
        .as_deref()
        .ok_or_else(|| CommandError::new(ErrorKind::Refused, "the Task has no Workspace"))?;
    cx.surface()
        .focus_workspace(&WorkspaceRef::new(workspace))?;
    Ok(SleepValue {
        task: req.task.as_str().to_owned(),
        r#box: task_view(&row, cx.config()).r#box,
        woke,
    })
}

#[cfg(test)]
mod tests;
