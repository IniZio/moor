//! The extra steps of `run --record`: refuse a checkout that cannot be reproduced, and wrap the
//! command so the Box runs it from a clean checkout of HEAD.

use std::path::Path;

use moor_core::boxscript::command_line;
use moor_core::boxscript::record::record_script;
use moor_core::{ErrorKind, RunReq, Sha};

use crate::cx::Cx;
use crate::error::CommandError;

/// Refuses option combinations a record run cannot honour.
///
/// # Errors
/// [`CommandError`] of kind `Usage` when `--record` is combined with `--no-sync`.
pub(super) fn usage(req: &RunReq) -> Result<(), CommandError> {
    if req.record.is_some() && req.no_sync {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "--record cannot be used with --no-sync",
        ));
    }
    Ok(())
}

/// Checks the worktree can back a record run and returns the HEAD sha the run is stored under.
///
/// The Box receives the working tree, so any uncommitted change is refused to keep the stored sha
/// true to what ran.
///
/// # Errors
/// [`CommandError`] when the worktree is missing, has uncommitted changes, or HEAD cannot be read.
pub(super) fn checkout(cx: &Cx, worktree: &Path) -> Result<Sha, CommandError> {
    let shown = worktree.display();
    if !worktree.is_dir() {
        return Err(CommandError::new(
            ErrorKind::Failed,
            format!("worktree {shown} is missing"),
        ));
    }
    let dirty = cx
        .git()
        .dirty(worktree)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
    if dirty {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("worktree {shown} has uncommitted changes; commit first"),
        ));
    }
    cx.git()
        .head(worktree)
        .map_err(|e| e.to_string())
        .and_then(|head| Sha::parse(&head).map_err(|e| e.to_string()))
        .map_err(|e| {
            CommandError::new(
                ErrorKind::Failed,
                format!("cannot read HEAD of {shown}: {e}"),
            )
        })
}

/// The argv a record run executes: the repo setup lines, then the command, in one script.
pub(super) fn wrap(setup: &[String], argv: &[String]) -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        record_script(setup, &command_line(argv)),
    ]
}

#[cfg(test)]
mod tests;
