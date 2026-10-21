//! The close decider: the data-loss gate and the release rules.

use crate::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::Decision;
use crate::domain::session_state::{SessionInput, SessionState};
use crate::name::BoxId;
use crate::response::Warning;

/// What the close op read before deciding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFacts {
    /// The box being closed.
    pub box_id: BoxId,
    /// Its Box lifecycle state.
    pub lifecycle: BoxLifecycle,
    /// The state of its primary Session.
    pub session: SessionState,
    /// Its Box state; `None` when no Box ever existed.
    pub box_state: Option<BoxState>,
    /// Whether a Driver handle is stored (kept even after a failed acquire).
    pub has_handle: bool,
    /// The box name, used in recovery commands.
    pub name: String,
    /// The box branch; `None` when it is not known.
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
    /// The box is already closed or was rejected.
    #[error("Box is already closed")]
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
    /// The Box holds work that is not pushed.
    #[error(
        "the Box holds work that is not pushed ({ahead} unpushed commit(s), \
         {dirty} uncommitted file(s)); to keep it {}; \
         or run `moor close {name} --force` to discard it",
        keep_hint(name, branch, *ahead)
    )]
    UnsavedInBox {
        /// The box name.
        name: String,
        /// The box branch, or a placeholder when unknown.
        branch: String,
        /// Commits not reachable from any remote-tracking ref or the base.
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

/// How to keep the unsaved work of Box `name`, which has `ahead` unpushed commits on `branch`.
fn keep_hint(name: &str, branch: &str, ahead: u32) -> String {
    let push = format!("`moor run {name} -- git push origin HEAD:refs/heads/{branch}`");
    if ahead == 0 {
        format!(
            "commit it in the Box, then run {push} \
             (inspect it first with `moor run {name} -- git status`)"
        )
    } else {
        format!(
            "run {push}; or run `moor run {name} -- git bundle create .moor/rescue.bundle HEAD --not --remotes`, \
             then `moor cp {name} .moor/rescue.bundle .`, \
             then `git fetch ./rescue.bundle HEAD:{branch}`"
        )
    }
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
    /// False when the box must stay open and the close fails.
    pub closes: bool,
}

/// Decides whether `f` may close, and what closing involves.
///
/// # Errors
/// [`Refusal::AlreadyClosed`] for a closed box; when `force` is false,
/// [`Refusal::UnpushedWork`] for unsaved host work, [`Refusal::UnsavedInBox`]
/// for work only in the Box, and [`Refusal::BoxUnchecked`] when the Box could
/// not be checked.
pub fn close_gate(f: &CloseFacts, force: bool) -> Result<ClosePlan, Refusal> {
    if !f.lifecycle.is_live() {
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
                    name: f.name.clone(),
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
        decision_close: Decision::new(f.box_id)
            .lifecycle(BoxLifecycleInput::Close)
            .session(SessionInput::End),
    })
}

/// A Gone Box is never released through the Driver; a Starting one has no handle yet.
const fn releasable(s: BoxState) -> bool {
    matches!(
        s,
        BoxState::Ready | BoxState::Asleep | BoxState::Failed | BoxState::ReleaseFailed
    )
}

/// Maps a release result to the decision that closes the Box.
///
/// A live Box (`Ready`/`Asleep`) that cannot be released keeps the Box open;
/// a `Failed` or `ReleaseFailed` Box does not hold the close back.
#[must_use]
pub fn release_outcome(box_id: BoxId, box_state: BoxState, r: ReleaseResult) -> ReleaseVerdict {
    let close = Decision::new(box_id)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
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
        message: "the Box is closed but was not deleted on the Driver; \
                  its handle kept for manual release"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests;
