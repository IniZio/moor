//! Creates the Task's worktree, replacing a safe one left by an earlier Task of the same name.

use std::path::Path;

use moor_core::seam::surface::{SurfaceError, Worktree};
use moor_core::{ErrorKind, StartReq};

use super::Facts;
use crate::cx::Cx;
use crate::error::CommandError;

/// Creates the worktree; a leftover on the Task's branch is removed first when it holds no
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
    match create() {
        Err(SurfaceError::Leftover { path }) => {
            clear(cx, &path, facts.from.as_deref())?;
            Ok(create()?)
        }
        other => Ok(other?),
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
                "the worktree {} was left by an earlier Task of this name and is kept: {why}",
                path.display()
            ),
        )
        .with_hint("push or remove that worktree, or pick another Task name"));
    }
    match cx.surface().remove_worktree(path) {
        Ok(()) | Err(SurfaceError::NotFound) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
