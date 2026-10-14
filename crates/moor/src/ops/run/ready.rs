//! What `run` checks before it touches the Box: the Task must hold a usable Box.

use std::path::PathBuf;

use moor_core::domain::boxes::BoxState;
use moor_core::domain::task::TaskState;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::ErrorKind;

use crate::error::CommandError;
use crate::ops::common::provision::recorded_workdir;
use crate::store::{Store, TaskRow};

/// Where a run executes.
pub(super) struct Target {
    /// The Box to run in.
    pub(super) handle: BoxHandle,
    /// Working directory in the Box; the driver's default when unrecorded.
    pub(super) workdir: Option<BoxPath>,
    /// The Task's worktree on the host.
    pub(super) worktree: PathBuf,
}

/// The Box and worktree a run uses, or the reason none can.
///
/// A Box the driver no longer knows is also recorded as a refused run.
pub(super) fn target(store: &Store, row: &TaskRow) -> Result<Target, CommandError> {
    let name = row.name.as_str();
    let failed = |msg: String| CommandError::new(ErrorKind::Failed, msg);
    if row.state == TaskState::WaitingForBox {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("{name} is waiting for a free box"),
        ));
    }
    let Some(boxed) = &row.boxed else {
        return Err(failed(format!("{name} has no box")));
    };
    match boxed.state {
        BoxState::Ready => {}
        BoxState::Gone => {
            store.refused_run(row.id)?;
            return Err(
                failed(format!("box of {name} is gone; run moor close {name}"))
                    .with_hint(format!("moor close {name}")),
            );
        }
        other => {
            return Err(failed(format!("box of {name} is {}", other.as_str())));
        }
    }
    let handle = boxed
        .handle
        .clone()
        .ok_or_else(|| failed(format!("box of {name} has no handle")))?;
    let workdir = boxed.workdir.as_deref().and_then(recorded_workdir);
    let worktree = PathBuf::from(row.worktree.as_deref().unwrap_or(&row.repo));
    Ok(Target {
        handle,
        workdir,
        worktree,
    })
}
