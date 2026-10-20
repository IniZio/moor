//! Syncs the kept worktree's branch and removes the worktree on close.

use std::path::Path;

use moor_core::seam::surface::SurfaceError;
use moor_core::Warning;

use crate::adapters::git::BranchSync;
use crate::cx::Cx;
use crate::store::BoxRow;

/// Brings the kept worktree and its local Box branch up to the remote tip, so they show the
/// work the agent pushed from the Box.
///
/// Only a fast-forward moves anything; every other outcome leaves the checkout alone and warns.
///
/// Returns false when the sync was skipped with a warning.
pub(super) fn sync_branch(cx: &Cx, row: &BoxRow) -> bool {
    let (Some(wt), Some(branch)) = (row.worktree.as_deref(), row.branch.as_deref()) else {
        return true;
    };
    let wt = Path::new(wt);
    if !wt.is_dir() {
        return true;
    }
    let why = match cx.git().sync_branch(wt, branch) {
        Ok(BranchSync::FastForwarded | BranchSync::UpToDate | BranchSync::NotPushed) => {
            return true;
        }
        Ok(BranchSync::Diverged) => format!(
            "the local branch {branch} and the remote one have each moved on; \
             merge or rebase by hand"
        ),
        Ok(BranchSync::Dirty) => {
            format!("the worktree has uncommitted changes; update {branch} by hand")
        }
        Ok(BranchSync::OffBranch) => {
            format!("the worktree does not have {branch} checked out; update it by hand")
        }
        Err(e) => format!("could not fetch {branch} from the remote: {e}"),
    };
    cx.warn(Warning {
        code: "branch_not_synced".to_owned(),
        message: format!("the worktree was left as it was: {why}"),
    });
    false
}

/// Removes the worktree moor created when its work is safe; otherwise keeps it and says why.
///
/// Returns whether the worktree is gone.
pub(super) fn remove_worktree(cx: &Cx, row: &BoxRow, synced: bool) -> bool {
    let Some(wt) = row.worktree.as_deref().filter(|_| !row.bound) else {
        return false;
    };
    let wt = Path::new(wt);
    let keep = |why: &str| {
        cx.warn(Warning {
            code: "worktree_kept".to_owned(),
            message: format!("the worktree was kept: {why}"),
        });
        false
    };
    if !synced {
        return keep("its branch was not synced");
    }
    let safe = cx
        .git()
        .unpushed(wt, row.base.as_deref())
        .map(|n| n == 0)
        .and_then(|clean| cx.git().dirty(wt).map(|d| clean && !d));
    match safe {
        Ok(true) => {}
        Ok(false) => return keep("it has unpushed or uncommitted work"),
        Err(e) => return keep(&format!("could not check it for unsaved work: {e}")),
    }
    match cx.surface().remove_worktree(wt) {
        Ok(()) | Err(SurfaceError::NotFound) => true,
        Err(e) => {
            cx.warn(Warning {
                code: "worktree_remove_failed".to_owned(),
                message: format!("the Box is closed but its worktree could not be removed: {e}"),
            });
            false
        }
    }
}
