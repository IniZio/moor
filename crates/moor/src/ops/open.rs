//! `open`: wake a Task's asleep Box, then focus its Workspace.
//!
//! Holds the Box lock while the Box resumes.

use moor_core::seam::proc::Tool;
use moor_core::seam::surface::WorkspaceRef;
use moor_core::view::sleep::SleepValue;
use moor_core::{BoxReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::wake::wake_box;

/// Run `open`.
pub(crate) fn run(cx: &Cx, req: &BoxReq) -> Result<SleepValue, CommandError> {
    wake_box(cx, req, "open", |row| {
        let workspace = row
            .workspace_id
            .as_deref()
            .ok_or_else(|| CommandError::new(ErrorKind::Refused, "the Task has no Workspace"))?;
        cx.surface()
            .focus_workspace(&WorkspaceRef::new(workspace))?;
        Ok(())
    })
}

/// Result of `connect`: the command the client runs to focus the Workspace.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ConnectValue {
    /// Logical program name; the client resolves the binary itself.
    pub tool: &'static str,
    /// Arguments for `tool`.
    pub args: Vec<String>,
    /// Whether this call resumed the Box.
    pub woke: bool,
}

/// Run `connect`: wake like `open`, but hand the focus command back
/// instead of running it.
pub(crate) fn connect(cx: &Cx, req: &BoxReq) -> Result<ConnectValue, CommandError> {
    let mut workspace = None;
    let slept = wake_box(cx, req, "connect", |row| {
        workspace =
            Some(row.workspace_id.clone().ok_or_else(|| {
                CommandError::new(ErrorKind::Refused, "the Task has no Workspace")
            })?);
        Ok(())
    })?;
    let workspace = workspace.unwrap_or_default();
    Ok(ConnectValue {
        tool: Tool::Herdr.program(),
        args: ["workspace", "focus", workspace.as_str()]
            .map(str::to_owned)
            .to_vec(),
        woke: slept.woke,
    })
}

#[cfg(test)]
mod tests;
