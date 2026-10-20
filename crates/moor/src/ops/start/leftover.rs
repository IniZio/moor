//! Creates the Box's worktree, replacing a safe one left by an earlier Box of the same name.

use std::path::Path;

use moor_core::seam::surface::{SurfaceError, Worktree};
use moor_core::{ErrorKind, StartReq};

use super::Facts;
use crate::cx::Cx;
use crate::error::CommandError;

/// Creates the worktree; a leftover on the Box's branch is removed first when it holds no
/// unpushed or uncommitted work, and refused otherwise.
pub(super) fn create_worktree(
    cx: &Cx,
    req: &StartReq,
    facts: &Facts,
) -> Result<Worktree, CommandError> {
    let create = || {
        cx.surface()
            .create_worktree(&facts.repo, &req.name, facts.from.as_deref())
    };
    let wt = match create() {
        Err(SurfaceError::Leftover { path }) => {
            clear(cx, &path, facts.from.as_deref())?;
            create()?
        }
        other => other?,
    };
    refuse_owned(cx, &wt)?;
    Ok(wt)
}

/// Refuses a worktree or branch the Surface returned that an open Box already holds.
fn refuse_owned(cx: &Cx, wt: &Worktree) -> Result<(), CommandError> {
    let path = wt.path.to_string_lossy();
    let owner = cx.store()?.open_boxes()?.into_iter().find(|t| {
        t.worktree.as_deref() == Some(path.as_ref()) || t.branch.as_deref() == Some(&wt.branch)
    });
    match owner {
        Some(t) => Err(CommandError::new(
            ErrorKind::Failed,
            format!(
                "the worktree {path} on branch {} belongs to open Box {}",
                wt.branch,
                t.name.as_str()
            ),
        )
        .with_hint("pick another Box name")),
        None => Ok(()),
    }
}

/// Removes the leftover worktree at `path` when its work is safe.
fn clear(cx: &Cx, path: &Path, base: Option<&str>) -> Result<(), CommandError> {
    let git = cx.git();
    let unsaved = match git.unpushed(path, base) {
        Ok(0) => match git.dirty(path) {
            Ok(false) => None,
            Ok(true) => Some("it has uncommitted files".to_owned()),
            Err(e) => Some(format!(
                "it could not be checked for uncommitted files: {e}"
            )),
        },
        Ok(n) => Some(format!("it has {n} unpushed commit(s)")),
        Err(e) => Some(format!("it could not be checked for unpushed commits: {e}")),
    };
    if let Some(why) = unsaved {
        return Err(CommandError::new(
            ErrorKind::Failed,
            format!(
                "the worktree {} was left by an earlier Box of this name and is kept: {why}",
                path.display()
            ),
        )
        .with_hint("push or remove that worktree, or pick another Box name"));
    }
    match cx.surface().remove_worktree(path) {
        Ok(()) | Err(SurfaceError::NotFound) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
