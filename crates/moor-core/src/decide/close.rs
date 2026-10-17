//! The close decider: the data-loss gate and the release rules.

use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::Decision;
use crate::domain::task::{TaskInput, TaskState};
use crate::name::TaskId;
use crate::response::Warning;

/// What the close op read before deciding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFacts {
    /// The task being closed.
    pub task: TaskId,
    /// Its current state.
    pub state: TaskState,
    /// Its Box state; `None` when no Box ever existed.
    pub box_state: Option<BoxState>,
    /// Whether a Driver handle is stored (kept even after a failed acquire).
    pub has_handle: bool,
    /// The task name, used in recovery commands.
    pub name: String,
    /// The task branch; `None` when it is not known.
    pub branch: Option<String>,
    /// Commits in the host worktree not pushed anywhere.
    pub unpushed: u32,
    /// Uncommitted changes in the host worktree.
    pub dirty: bool,
    /// What the Box itself holds.
    pub box_check: BoxCheck,
}

/// What the Box itself holds, as checked before close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxCheck {
    /// Not checked: forced, or no reachable Box that could hold work.
    Skipped,
    /// The Box holds nothing that is not on the host.
    Clean,
    /// Commits not pushed or reachable from the host, and uncommitted files.
    Unsaved {
        /// Commits not pushed or reachable from the host.
        ahead: u32,
        /// Uncommitted files.
        dirty: u32,
    },
    /// The Box could not be checked; the reason.
    Unknown(String),
}

/// What a permitted close does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosePlan {
    /// Ask the Driver to release the Box; the result goes to [`release_outcome`].
    pub release_box: bool,
    /// Close the Workspace; a Workspace that is already gone is fine.
    pub close_workspace: bool,
    /// The decision to apply when no release runs.
    pub decision_close: Decision,
}

/// Why a close is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The task is already closed or was rejected.
    #[error("task is already closed")]
    AlreadyClosed,
    /// Work would be lost.
    #[error(
        "the host worktree has work that is not saved ({unpushed} unpushed commit(s), {}); \
         push it, or pass --force to close anyway",
        if *dirty { "uncommitted changes" } else { "no uncommitted changes" }
    )]
    UnpushedWork {
        /// Unpushed commits in the host worktree.
        unpushed: u32,
        /// Whether the host worktree is dirty.
        dirty: bool,
    },
    /// The Box holds work that is not on the host.
    #[error(
        "the Box holds work that is not on the host ({ahead} unpushed commit(s), \
         {dirty} uncommitted file(s)); to keep it {}; \
         or run `moor close {task} --force` to discard it",
        if *ahead == 0 {
            format!(
                "commit it in the Box and, if the Box has the moor remote, run \
                 `git push moor HEAD:refs/heads/{branch}` there \
                 (inspect it first with `moor run {task} --no-sync -- git status`)"
            )
        } else {
            format!(
                "run `moor run {task} --no-sync -- git bundle create .moor/rescue.bundle HEAD --not --remotes`, \
                 then `moor cp {task} .moor/rescue.bundle .`, \
                 then `git fetch ./rescue.bundle HEAD:{branch}`; or push from the Box"
            )
        }
    )]
    UnsavedInBox {
        /// The task name.
        task: String,
        /// The task branch, or a placeholder when unknown.
        branch: String,
        /// Commits not pushed or reachable from the host.
        ahead: u32,
        /// Uncommitted files.
        dirty: u32,
    },
    /// The Box could not be checked for unsaved work.
    #[error(
        "could not check the Box for unsaved work: {reason}; \
         pass --force to close anyway and discard any work in the Box"
    )]
    BoxUnchecked {
        /// Why the check failed.
        reason: String,
    },
}

/// What the Driver said to a release request, without its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseResult {
    /// The Box was deleted.
    Deleted,
    /// The Driver reported the Box already absent.
    AlreadyGone,
    /// Any other Driver error, before any release step ran.
    Failed,
    /// The release steps ran but left part of the Box behind.
    Left,
}

/// What to do after a release request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseVerdict {
    /// The decision to apply; `None` leaves the store untouched.
    pub decision: Option<Decision>,
    /// A moor-worded warning for the caller.
    pub warning: Option<Warning>,
    /// False when the task must stay open and the close fails.
    pub closes: bool,
}

/// Decides whether `f` may close, and what closing involves.
///
/// # Errors
/// [`Refusal::AlreadyClosed`] for a closed task; when `force` is false,
/// [`Refusal::UnpushedWork`] for unsaved host work, [`Refusal::UnsavedInBox`]
/// for work only in the Box, and [`Refusal::BoxUnchecked`] when the Box could
/// not be checked.
pub fn close_gate(f: &CloseFacts, force: bool) -> Result<ClosePlan, Refusal> {
    if !f.state.is_live() {
        return Err(Refusal::AlreadyClosed);
    }
    if !force {
        if f.unpushed > 0 || f.dirty {
            return Err(Refusal::UnpushedWork {
                unpushed: f.unpushed,
                dirty: f.dirty,
            });
        }
        match &f.box_check {
            BoxCheck::Unsaved { ahead, dirty } if *ahead > 0 || *dirty > 0 => {
                return Err(Refusal::UnsavedInBox {
                    task: f.name.clone(),
                    branch: f.branch.clone().unwrap_or_else(|| "<branch>".to_owned()),
                    ahead: *ahead,
                    dirty: *dirty,
                });
            }
            BoxCheck::Unknown(reason) => {
                return Err(Refusal::BoxUnchecked {
                    reason: reason.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(ClosePlan {
        release_box: f.has_handle && f.box_state.is_some_and(releasable),
        close_workspace: true,
        decision_close: Decision::new(f.task).task_input(TaskInput::Close),
    })
}

/// A Gone Box is never released through the Driver; a Starting one has no handle yet.
const fn releasable(s: BoxState) -> bool {
    matches!(
        s,
        BoxState::Ready | BoxState::Asleep | BoxState::Failed | BoxState::ReleaseFailed
    )
}

/// Maps a release result to the decision that closes the task.
///
/// - `Deleted` / `AlreadyGone`: the Box is released and the task closes.
/// - `Left`: lease rows always remain. A plain close keeps the task open; `force` closes it
///   with a warning that lists `left` and says they need manual cleanup.
/// - `Failed` (Driver error), plain close with `rows_left`: the task stays open and the next
///   plain close runs the release again.
/// - `Failed`, plain close without `rows_left`: nothing is left to retry, so the task closes
///   with a warning that the Box handle needs manual release.
/// - `Failed` with `force`: the task closes with a warning naming the Driver failure.
///
/// Whenever the release did not finish the Box goes to `ReleaseFailed` (or stays there).
#[must_use]
pub fn release_outcome(
    id: TaskId,
    state: BoxState,
    result: ReleaseResult,
    force: bool,
    left: &[String],
    rows_left: bool,
) -> ReleaseVerdict {
    let close = Decision::new(id).task_input(TaskInput::Close);
    if matches!(result, ReleaseResult::Deleted | ReleaseResult::AlreadyGone) {
        return ReleaseVerdict {
            decision: Some(close.box_input(BoxInput::Released)),
            warning: None,
            closes: true,
        };
    }
    let cause = if result == ReleaseResult::Left {
        format!("the Box release left: {}", left.join("; "))
    } else {
        "the Driver could not release the Box".to_owned()
    };
    // `ReleaseFailed` has no `ReleaseErr` edge: the Box is already recorded.
    let record = |d: Decision| {
        if state == BoxState::ReleaseFailed {
            d
        } else {
            d.box_input(BoxInput::ReleaseErr { cause })
        }
    };
    if force {
        ReleaseVerdict {
            decision: Some(record(close)),
            warning: Some(release_warning(result, left)),
            closes: true,
        }
    } else if result == ReleaseResult::Failed && !rows_left {
        ReleaseVerdict {
            decision: Some(record(close)),
            warning: Some(handle_warning()),
            closes: true,
        }
    } else {
        ReleaseVerdict {
            decision: (state != BoxState::ReleaseFailed).then(|| record(Decision::new(id))),
            warning: None,
            closes: false,
        }
    }
}

/// The notice for a plain close whose Driver release failed with no lease rows to retry.
fn handle_warning() -> Warning {
    Warning {
        code: "box_release_failed".to_owned(),
        message: "the task is closed but the Driver could not release the Box; \
                  the Box handle needs manual release"
            .to_owned(),
    }
}

/// The caller-facing notice for a forced close that left part of the Box behind.
fn release_warning(result: ReleaseResult, left: &[String]) -> Warning {
    let what = if result == ReleaseResult::Left {
        format!("left on the Box: {}", left.join("; "))
    } else {
        "the Driver could not release the Box".to_owned()
    };
    Warning {
        code: "box_release_failed".to_owned(),
        message: format!(
            "the task is closed but its Box was not fully released; {what}; \
             these need manual cleanup on the Box"
        ),
    }
}

#[cfg(test)]
mod tests;
