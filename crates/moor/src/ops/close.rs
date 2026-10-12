//! `moor close`: gate, release the Box, then close the Workspace.

use std::path::Path;

use moor_core::decide::close::{close_gate, release_outcome, CloseFacts, ClosePlan, ReleaseResult};
use moor_core::seam::driver::Released;
use moor_core::seam::surface::WorkspaceRef;
use moor_core::view::CloseView;
use moor_core::{BoxHandle, CloseReq, ErrorKind, TaskName, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::promote::promote_waiting;
use crate::ops::common::view::task_view;
use crate::store::{BoxRow, Store, TaskRow};

/// Closes a task: the data-loss gate, Box release, Workspace close.
///
/// The Box is released before the Workspace closes. The worktree is never
/// removed; it belongs to the user's checkout. Waiting tasks are then promoted
/// into the freed slot.
pub(crate) fn run(cx: &Cx, req: &CloseReq) -> Result<CloseView, CommandError> {
    let store = cx.store()?;
    let row = find(store, &req.task)?;
    let plan = close_gate(&facts(cx, &row)?, req.force)?;
    let box_deleted = if plan.release_box {
        release(cx, store, &row)?
    } else {
        store.apply(plan.decision_close.clone())?;
        false
    };
    let workspace_closed = close_workspace(cx, &row, &plan);
    let provider = row.boxed.as_ref().map(|b| b.provider.clone());
    let closed = store.task(row.id)?.unwrap_or(row);
    let promoted = provider.map_or_else(Vec::new, |p| {
        promote_waiting(cx, store, &p, "close")
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect()
    });
    Ok(CloseView {
        task: task_view(&closed),
        box_deleted,
        workspace_closed,
        agent_stopped: false,
        transcript: None,
        worktree_kept: true,
        promoted,
    })
}

/// The open task by name, else the newest closed one (the gate refuses it).
fn find(store: &Store, name: &TaskName) -> Result<TaskRow, CommandError> {
    if let Some(row) = store.open_task_by_name(name)? {
        return Ok(row);
    }
    store.latest_task_by_name(name)?.ok_or_else(|| {
        CommandError::new(
            ErrorKind::Failed,
            format!("no task named {}", name.as_str()),
        )
    })
}

/// The Driver handle to release: the acquired one, else one a failed acquire left.
const fn held(b: &BoxRow) -> Option<&BoxHandle> {
    match (&b.handle, &b.attempted_handle) {
        (Some(h), _) | (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// Reads the unsaved-work facts the gate decides on.
fn facts(cx: &Cx, row: &TaskRow) -> Result<CloseFacts, CommandError> {
    let (mut unpushed, mut dirty) = (0, false);
    if let Some(wt) = row.worktree.as_deref().map(Path::new) {
        let failed = |e| CommandError::new(ErrorKind::Failed, e);
        unpushed = cx.git().unpushed(wt, row.base.as_deref()).map_err(failed)?;
        dirty = cx.git().dirty(wt).map_err(failed)?;
    }
    Ok(CloseFacts {
        task: row.id,
        state: row.state,
        box_state: row.boxed.as_ref().map(|b| b.state),
        has_handle: row.boxed.as_ref().is_some_and(|b| held(b).is_some()),
        unpushed,
        dirty,
    })
}

/// Asks the Driver to release the Box and records the verdict.
///
/// Returns whether the Box is now deleted.
fn release(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    let Some(b) = row.boxed.as_ref() else {
        return Ok(false);
    };
    let Some(handle) = held(b) else {
        return Ok(false);
    };
    let (result, cause) = match cx.driver().release(handle) {
        Ok(Released::Deleted | Released::AlreadyGone) => (ReleaseResult::Deleted, None),
        Err(e) => (ReleaseResult::Failed, Some(e)),
    };
    let verdict = release_outcome(row.id, b.state, result);
    if let Some(d) = verdict.decision {
        store.apply(d)?;
    }
    if let Some(w) = verdict.warning {
        cx.warn(w);
    }
    if !verdict.closes {
        let why = cause.map_or_else(
            || "the Box could not be released".to_owned(),
            |e| e.to_string(),
        );
        return Err(
            CommandError::new(ErrorKind::Failed, format!("{why}; the task stays open"))
                .with_hint("try `moor close` again, or release the Box by hand"),
        );
    }
    Ok(result == ReleaseResult::Deleted)
}

/// Closes the Workspace; a missing one is success, other failures warn.
fn close_workspace(cx: &Cx, row: &TaskRow, plan: &ClosePlan) -> bool {
    let Some(id) = row.workspace_id.as_deref().filter(|_| plan.close_workspace) else {
        return false;
    };
    match cx.surface().close_workspace(&WorkspaceRef::new(id)) {
        Ok(()) | Err(moor_core::seam::surface::SurfaceError::NotFound) => true,
        Err(_) => {
            cx.warn(Warning {
                code: "workspace_close_failed".to_owned(),
                message: "the task is closed but its Workspace could not be closed; \
                          close it by hand"
                    .to_owned(),
            });
            false
        }
    }
}

#[cfg(test)]
mod tests;
