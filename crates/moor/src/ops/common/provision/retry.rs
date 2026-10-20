//! Re-runs the steps that never finished on a ready Box whose last start ended in setup-failed.

use std::path::Path;

use moor_core::config::TaskSettings;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::{BoxHandle, BoxPath, ErrorKind};

use super::{recorded_workdir, run_and_record, Provisioned, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// The kept Box's handle and workdir, when the Task failed setup on a Box that is still ready.
fn kept_box(row: &BoxRow) -> Option<(&BoxHandle, BoxPath)> {
    let boxed = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    if row.lifecycle != BoxLifecycle::SetupFailed {
        return None;
    }
    let workdir = boxed.workdir.as_deref().and_then(recorded_workdir)?;
    Some((boxed.handle.as_ref()?, workdir))
}

/// Whether the Task failed setup on a Box that is still ready, so its steps run again in it.
pub(super) fn kept_box_retry(row: &BoxRow) -> bool {
    kept_box(row).is_some()
}

/// Runs sync, setup and dotfiles in the kept Box under the setup intent, then marks it ready.
///
/// The caller holds the Box lock and passes the row read under it. No Box is acquired. The
/// intent is cleared on success and by the failure record, so a crash mid-run leaves it for
/// dead-owner recovery.
pub(super) fn retry_in_box(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &TaskSettings,
) -> Result<Provisioned, CommandError> {
    let Some((handle, workdir)) = kept_box(row) else {
        return Err(CommandError::new(ErrorKind::Internal, "kept Box vanished"));
    };
    let begin = Decision::new(row.id)
        .lifecycle(BoxLifecycleInput::Retry)
        .intent(IntentChange::Set(Intent::Setup {
            pid: std::process::id(),
        }));
    store.apply(begin)?;
    let target = SetupTarget {
        handle,
        workdir: &workdir,
        worktree: Path::new(row.worktree.as_deref().unwrap_or(&row.repo)),
    };
    run_and_record(cx, store, row.id, true, &target, cfg)
}
