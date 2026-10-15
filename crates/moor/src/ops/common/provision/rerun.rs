//! Chooses how a setup re-run syncs the worktree into a Box that already exists.

use moor_core::domain::run::SyncMode;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, TaskRow};

/// The sync for a setup re-run in the Task's existing Box.
///
/// A sync resets the Box checkout to the host's: HEAD, branches and remote-tracking refs move
/// even when every commit is pushed. A Task that has had an agent session is therefore never
/// synced; the re-run warns instead.
pub(super) fn rerun_sync(cx: &Cx, store: &Store, row: &TaskRow) -> Result<SyncMode, CommandError> {
    if !store.has_session(row.id)? {
        return Ok(SyncMode::Full);
    }
    cx.warn(Warning {
        code: "setup_rerun_unsynced".to_owned(),
        message: "setup re-ran without syncing: the Box keeps its own commits and files".to_owned(),
    });
    Ok(SyncMode::None)
}
