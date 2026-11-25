//! The branch a place's work Box starts on: its task's, so a Box started the next day checks out
//! the commits the task's earlier Boxes pushed, and its pushes reach the same pull request.

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::{BoxId, ErrorKind};

use crate::error::CommandError;
use crate::store::Store;

/// The insert of `new`: on the branch of the first Box place `task` started, when it started
/// one, else on a branch named by the new Box's id.
///
/// # Errors
/// Refused while a Box of the task is still open on that branch, naming it; or a store failure.
pub(super) fn insert(
    store: &Store,
    task: Option<BoxId>,
    new: NewBox,
) -> Result<Decision, CommandError> {
    let Some(task) = task.map(|p| store.task_branch(p)).transpose()?.flatten() else {
        return Ok(Decision::insert(new).branch_by_id());
    };
    if let Some(open) = task.open {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!(
                "Box {} is still open on this task's branch {}; run commands in it with \
                 exec_in_box, or close it with close_box before starting another",
                open.as_str(),
                task.branch
            ),
        ));
    }
    Ok(Decision::insert(NewBox {
        spec: SpecFields {
            branch: Some(task.branch),
            ..new.spec
        },
        ..new
    }))
}
