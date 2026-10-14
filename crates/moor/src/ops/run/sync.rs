//! Whether a run syncs the host checkout into the Box first.
//!
//! A sync re-seeds the Box's `.git` from the host and copies files over, which discards commits
//! and changes made in the Box. It happens only when the Box holds nothing that would be lost.

use moor_core::domain::run::SyncMode;
use moor_core::{ErrorKind, RunReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxwork::{self, BoxWork};
use crate::store::{Store, TaskRow};

/// Refuses `--sync` together with `--no-sync`.
///
/// # Errors
/// [`CommandError`] of kind `Usage` when both flags are given.
pub(super) fn usage(req: &RunReq) -> Result<(), CommandError> {
    if req.sync && req.no_sync {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "--sync and --no-sync cannot be used together",
        ));
    }
    Ok(())
}

/// Picks the sync mode for a run.
///
/// `--no-sync` never syncs. `--sync` syncs only a Box with no unsaved work. With neither flag, a
/// Task that has had an agent session is not synced, since the agent's work lives in the Box.
///
/// # Errors
/// [`CommandError`] of kind `Usage` for conflicting flags, `Refused` when `--sync` would discard
/// Box work or the Box cannot be probed, or a store failure.
pub(super) fn choose(
    cx: &Cx,
    store: &Store,
    req: &RunReq,
    row: &TaskRow,
) -> Result<SyncMode, CommandError> {
    usage(req)?;
    if req.no_sync {
        return Ok(SyncMode::None);
    }
    if !req.sync {
        return Ok(if store.has_session(row.id)? {
            SyncMode::None
        } else {
            SyncMode::Full
        });
    }
    let work = row.boxed.as_ref().map_or_else(
        || BoxWork::Unknown("the task has no box".to_owned()),
        |boxed| boxwork::probe(cx, row, boxed),
    );
    match work {
        BoxWork::Clean => Ok(SyncMode::Full),
        BoxWork::Unsaved { ahead, dirty } => Err(refused(format!(
            "the Box has unsaved work: {ahead} commits the host lacks, {dirty} changed paths"
        ))),
        BoxWork::Unknown(reason) => Err(refused(format!(
            "cannot tell whether the Box has unsaved work: {reason}"
        ))),
    }
}

/// A refusal that points at both ways forward.
fn refused(msg: String) -> CommandError {
    CommandError::new(ErrorKind::Refused, msg)
        .with_hint("run with --no-sync to use the Box as it is")
        .with_hint(
            "save the Box work first: push it from the Box, or `git bundle create` it in the Box \
             and copy it out with `moor cp`",
        )
}
