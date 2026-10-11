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
    /// Commits in the Box not pushed anywhere.
    pub unpushed: u32,
    /// Uncommitted changes in the Box.
    pub dirty: bool,
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
        "the Box has work that is not saved ({unpushed} unpushed commit(s), {}); \
         push it, or pass --force to close anyway",
        if *dirty { "uncommitted changes" } else { "no uncommitted changes" }
    )]
    UnpushedWork {
        /// Unpushed commits in the Box.
        unpushed: u32,
        /// Whether the working tree is dirty.
        dirty: bool,
    },
}

/// What the Driver said to a release request, without its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseResult {
    /// The Box was deleted.
    Deleted,
    /// The Driver reported the Box already absent.
    AlreadyGone,
    /// Any other Driver error.
    Failed,
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
/// [`Refusal::AlreadyClosed`] for a closed task; [`Refusal::UnpushedWork`]
/// when work is unsaved and `force` is false.
pub fn close_gate(f: &CloseFacts, force: bool) -> Result<ClosePlan, Refusal> {
    if !f.state.is_live() {
        return Err(Refusal::AlreadyClosed);
    }
    if !force && (f.unpushed > 0 || f.dirty) {
        return Err(Refusal::UnpushedWork {
            unpushed: f.unpushed,
            dirty: f.dirty,
        });
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
/// A live Box (`Ready`/`Asleep`) that cannot be released keeps the task open;
/// a `Failed` or `ReleaseFailed` Box does not hold the task back.
#[must_use]
pub fn release_outcome(task: TaskId, box_state: BoxState, r: ReleaseResult) -> ReleaseVerdict {
    let close = Decision::new(task).task_input(TaskInput::Close);
    match r {
        ReleaseResult::Deleted | ReleaseResult::AlreadyGone => ReleaseVerdict {
            decision: Some(close.box_input(BoxInput::Released)),
            warning: None,
            closes: true,
        },
        ReleaseResult::Failed => match box_state {
            BoxState::Ready | BoxState::Asleep => ReleaseVerdict {
                decision: None,
                warning: None,
                closes: false,
            },
            _ => ReleaseVerdict {
                decision: Some(failed_close(close, box_state)),
                warning: Some(release_warning()),
                closes: true,
            },
        },
    }
}

/// Records `ReleaseFailed` unless the Box is already there.
fn failed_close(close: Decision, from: BoxState) -> Decision {
    if from == BoxState::ReleaseFailed {
        return close;
    }
    close.box_input(BoxInput::ReleaseErr {
        cause: "the Driver could not release the Box".to_owned(),
    })
}

/// The caller-facing notice for an unreleased Box.
fn release_warning() -> Warning {
    Warning {
        code: "box_release_failed".to_owned(),
        message: "the task is closed but its Box was not released; \
                  release it by hand to stop paying for it"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests;
