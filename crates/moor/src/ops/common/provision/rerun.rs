//! Decides whether a setup re-run is wanted, and how it syncs the worktree into a Box that already exists.

use moor_core::decide::intent::setup_dead;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::TaskState;
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

/// Whether the Task's Box is ready but its repo setup failed or its owner died.
pub(super) fn setup_wanted(cx: &Cx, row: &TaskRow) -> bool {
    if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Ready) {
        return false;
    }
    if let Some(intent) = &row.intent {
        return setup_dead(intent, cx.proc().alive(intent.pid()));
    }
    matches!(row.state, TaskState::Working | TaskState::Idle) && row.setup_failed_at.is_some()
}
