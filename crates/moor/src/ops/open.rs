//! `open`: wake a Task's asleep Box, then focus its Workspace.
//!
//! Holds the Box lock while the Box resumes.

use moor_core::seam::surface::WorkspaceRef;
use moor_core::view::sleep::SleepValue;
use moor_core::{ErrorKind, TaskReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::wake::wake_task;

/// Run `open`.
pub(crate) fn run(cx: &Cx, req: &TaskReq) -> Result<SleepValue, CommandError> {
    wake_task(cx, req, "open", |row| {
        let workspace = row
            .workspace_id
            .as_deref()
            .ok_or_else(|| CommandError::new(ErrorKind::Refused, "the Task has no Workspace"))?;
        cx.surface()
            .focus_workspace(&WorkspaceRef::new(workspace))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests;
