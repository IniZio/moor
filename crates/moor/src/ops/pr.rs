//! The open pull request of a live Task's branch.

use std::path::Path;

use moor_core::BoxName;

use crate::cx::Cx;
use crate::ops::common::proxy::origin;

#[cfg(test)]
mod tests;

/// The web URL of the open pull request on the live Task `task`'s branch, or `None` when the
/// Task, its branch, worktree origin or forge repo is unknown or no PR is open.
///
/// # Errors
/// The message of a name, store or forge failure; it never carries a token.
pub(crate) fn open_pr_url(cx: &Cx, task: &str) -> Result<Option<String>, String> {
    let name = BoxName::parse_allow_bare(task).map_err(|e| e.to_string())?;
    let store = cx.store().map_err(|e| e.error.to_string())?;
    let Some(row) = store.open_task_by_name(&name).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let (Some(branch), Some(worktree)) = (row.branch, row.worktree) else {
        return Ok(None);
    };
    let Some(origin) = origin(cx, Path::new(&worktree)) else {
        return Ok(None);
    };
    let Some(repo) = cx.forge().repo_of(&origin) else {
        return Ok(None);
    };
    cx.forge()
        .open_pr(&repo, &branch)
        .map_err(|e| e.to_string())
}
