//! Re-applies the dotfiles to a ready Box whose last start ended in setup-failed.

use std::path::Path;

use moor_core::config::Flags;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::domain::task::{SetupOutcome, TaskInput};

use super::outcome::{dotfiles_failed, with_egress_hint};
use super::Provisioned;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::dotfiles::{self, DotfilesRun};
use crate::ops::common::facts::settings;
use crate::ops::common::setup::SetupRun;
use crate::store::{Store, TaskRow};

/// Runs the dotfiles step again under the setup intent, then marks the Box ready.
///
/// The caller holds the Box lock and passes the row read under it. The intent is cleared on
/// success and by the failure record, so a crash mid-install leaves it for dead-owner recovery.
pub(super) fn retry_dotfiles(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
) -> Result<Provisioned, CommandError> {
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    let cfg = settings(cx, Path::new(at), &Flags::default())?;
    let begin = Decision::new(row.id)
        .task_input(TaskInput::Retry)
        .intent(IntentChange::Set(Intent::Setup {
            pid: std::process::id(),
        }));
    store.apply(begin)?;
    let handle = row.boxed.as_ref().and_then(|b| b.handle.as_ref());
    let run = handle.map_or(Ok(DotfilesRun::Skipped), |h| {
        dotfiles::apply(cx, h, row.id, Path::new(at), &cfg)
    });
    let dotfiles = match run {
        Ok(run) => run,
        Err(f) => {
            return Err(dotfiles_failed(
                cx,
                store,
                row.id,
                true,
                None,
                with_egress_hint(cx, &cfg, f),
            ))
        }
    };
    let ok = TaskInput::BoxReady {
        setup: SetupOutcome::Ok,
    };
    store.apply(
        Decision::new(row.id)
            .task_input(ok)
            .intent(IntentChange::Clear),
    )?;
    Ok(Provisioned {
        setup: SetupRun::Skipped,
        dotfiles,
    })
}
