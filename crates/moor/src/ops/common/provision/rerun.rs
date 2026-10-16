//! Decides whether a setup re-run is wanted, and how it syncs the worktree into a Box that already exists.

use moor_core::config::Flags;
use moor_core::decide::intent::setup_dead;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::TaskState;
use moor_core::Warning;
use std::path::Path;

use super::outcome::sync_failed;
use super::{moor_yaml, recorded_workdir};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxwork::{self, BoxWork};
use crate::ops::common::facts::resolved;
use crate::ops::common::setup;
use crate::store::{Store, TaskRow};

/// The sync for a start that re-runs in the Task's existing Box.
///
/// A sync resets the Box checkout to the host's: HEAD, branches and remote-tracking refs move
/// even when every commit is pushed. A Task that has had an agent session is therefore synced
/// only when the Box holds no work that exists nowhere else, or has no git checkout at all
/// (an earlier sync failed); otherwise the sync is skipped with a warning.
pub(super) fn rerun_sync(cx: &Cx, store: &Store, row: &TaskRow) -> Result<SyncMode, CommandError> {
    if !store.has_session(row.id)? {
        return Ok(SyncMode::Full);
    }
    let Some(boxed) = row.boxed.as_ref() else {
        return Ok(SyncMode::None);
    };
    let why = match boxwork::probe(cx, row, boxed) {
        BoxWork::Clean | BoxWork::NoCheckout => return Ok(SyncMode::Full),
        BoxWork::Unsaved { ahead, dirty } => {
            format!("the Box has {ahead} commits the host lacks and {dirty} changed paths")
        }
        BoxWork::Gone => "the Box is gone".to_owned(),
        BoxWork::Unknown(reason) => {
            format!("cannot tell whether the Box has unsaved work: {reason}")
        }
    };
    cx.warn(Warning {
        code: "resync_skipped".to_owned(),
        message: format!("did not sync the checkout into the Box: {why}"),
    });
    Ok(SyncMode::None)
}

/// What a caller knows that decides a re-sync of the checkout.
pub(crate) struct ResyncGates<'a> {
    /// Whether the caller is about to launch the agent; asked only if the re-sync fails.
    pub launching: &'a dyn Fn() -> Result<bool, CommandError>,
    /// Whether an agent session is still running.
    pub agent_live: &'a dyn Fn() -> Result<bool, CommandError>,
}

/// Syncs the host checkout into a ready Box when no setup re-run is going to.
///
/// Only the sync happens. A live agent does not respect the Box lock and may be writing files, so
/// no sync is made while an agent session is live. A failed sync ends the start like a fresh
/// one's when the agent is about to launch; otherwise it is a warning and the start goes on, so
/// the user can retry.
///
/// # Errors
/// The sync failed while launching, or the repo config could not be read.
pub(super) fn resync(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    trusted: bool,
    gates: &ResyncGates,
) -> Result<(), CommandError> {
    let Some(boxed) = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready) else {
        return Ok(());
    };
    let (Some(handle), Some(workdir)) = (
        boxed.handle.as_ref(),
        boxed.workdir.as_deref().and_then(recorded_workdir),
    ) else {
        return Ok(());
    };
    if (gates.agent_live)()? {
        cx.warn(Warning {
            code: "resync_skipped".to_owned(),
            message: "did not sync the checkout into the Box: an agent session is still running"
                .to_owned(),
        });
        return Ok(());
    }
    if rerun_sync(cx, store, row)? != SyncMode::Full {
        return Ok(());
    }
    let repo = moor_yaml::for_teardown(cx, row, trusted)?;
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let Err(reason) = setup::sync_checkout(cx, handle, &workdir, worktree, &cfg) else {
        return Ok(());
    };
    if (gates.launching)()? {
        let advance = row.state == TaskState::SettingUp;
        return Err(sync_failed(cx, store, row.id, advance, reason));
    }
    cx.warn(Warning {
        code: "resync_failed".to_owned(),
        message: format!("{reason}; run `moor start` again"),
    });
    Ok(())
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
