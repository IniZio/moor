//! Facts every task op needs, built once per command.

use std::path::Path;

use moor_core::config::{Flags, TaskSettings};

use crate::cx::Cx;
use crate::error::CommandError;

/// Settings for one command: user config from Cx, repo config worktree-first, flags.
///
/// # Errors
/// A bad repo config or an unresolvable setting, as a failed command.
pub(crate) fn settings(
    cx: &Cx,
    worktree: &Path,
    flags: &Flags,
) -> Result<TaskSettings, CommandError> {
    let repo = cx.repo_config(worktree)?;
    Ok(TaskSettings::resolve(cx.config(), repo.as_ref(), flags))
}

#[cfg(test)]
mod tests;
