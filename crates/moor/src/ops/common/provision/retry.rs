//! Re-runs the steps that never finished on a ready Box whose last start ended in setup-failed.

use moor_core::config::BoxSettings;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::{BoxHandle, BoxPath, ErrorKind};

use moor_core::config::Flags;
use moor_core::BoxId;

use super::rerun::record_interrupted;
use super::{layers, recorded_workdir, reread, run_and_record, Provisioned, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::facts::settings_of;
use crate::ops::common::{cache, setup};
use crate::store::{BoxRow, Store};

/// The handle and workdir of the machine `row` holds, when it is ready.
fn machine(row: &BoxRow) -> Option<(&BoxHandle, BoxPath)> {
    let boxed = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    let workdir = boxed.workdir.as_deref().and_then(recorded_workdir)?;
    Some((boxed.handle.as_ref()?, workdir))
}

/// The kept Box's handle and workdir, when the Box failed setup on a machine that is still ready.
fn kept_box(row: &BoxRow) -> Option<(&BoxHandle, BoxPath)> {
    machine(row).filter(|_| row.lifecycle == BoxLifecycle::SetupFailed)
}

/// Whether the Box failed setup on a machine that is still ready, so its steps run again in it.
pub(super) fn kept_box_retry(row: &BoxRow) -> bool {
    kept_box(row).is_some()
}

/// Runs the clone, setup and dotfiles in the kept Box under the setup intent, then marks it ready.
///
/// The caller holds the Box lock and passes the row read under it. No Box is acquired. The
/// intent is cleared on success and by the failure record, so a crash mid-run leaves it for
/// dead-owner recovery.
pub(super) fn retry_in_box(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
) -> Result<Provisioned, CommandError> {
    let begin = Decision::new(row.id)
        .lifecycle(BoxLifecycleInput::Retry)
        .intent(IntentChange::Set(Intent::Setup {
            pid: std::process::id(),
        }));
    store.apply(begin)?;
    set_up_in_machine(cx, store, row, cfg, true)
}

/// Follows again the setup a controller restart cut off in Box `id`, which this process claimed:
/// each setup script that ran or is running is attached to rather than run again, and the Box
/// moves on as the start would have. The Box lock is held meanwhile; a Box that is no longer
/// claimed is left alone.
pub(crate) fn resume_setup(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
    let row = super::reread(store, id)?;
    let mine = Some(Intent::Setup {
        pid: std::process::id(),
    });
    if row.intent != mine || row.lifecycle != BoxLifecycle::SettingUp {
        return Ok(());
    }
    let cfg = settings_of(cx, &row, &Flags::default())?;
    if let Some((handle, _)) = machine(&row) {
        setup::await_reachable(cx, handle)?;
    }
    match set_up_in_machine(cx, store, &row, &cfg, false) {
        Ok(_) => Ok(()),
        // The intent stays, so the next process claims this setup again.
        Err(e) if cx.env().draining().is_cancelled() => Err(e),
        Err(e) => {
            record_interrupted(cx, store, &reread(store, id)?)?;
            Err(e)
        }
    }
}

/// Runs the clone, setup and dotfiles in the machine `row` holds, then marks it ready; `fork`
/// first moves a retried Box onto the checkpoint nearest its failed line.
fn set_up_in_machine(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    fork: bool,
) -> Result<Provisioned, CommandError> {
    let (Some((handle, workdir)), Some(held)) = (machine(row), row.boxed.as_ref()) else {
        return Err(CommandError::new(ErrorKind::Internal, "kept Box vanished"));
    };
    let provider = held.provider.as_str();
    let plan = cache::derived(cx, store, row, cfg, &held.provider);
    let mut layered = layers::plan(cx, store, row, provider, cfg, &plan);
    let forked = match layered.as_mut() {
        Some(layered) if fork => {
            layers::stand_on_rerun(store, row.id, layered)?;
            layers::resume(cx, store, row, handle, cfg, layered)?
        }
        Some(layered) => {
            layers::stand_on_warm_start(store, row.id, layered)?;
            None
        }
        None => None,
    };
    let log_dir = setup::log_dir(cx.state_dir()?, row.id);
    let target = SetupTarget {
        handle: forked.as_ref().unwrap_or(handle),
        provider,
        workdir: &workdir,
        repo: row.repo.as_ref(),
        log_dir: &log_dir,
        tree_of: None,
        layered: layered.as_ref(),
    };
    run_and_record(cx, store, row.id, true, &target, cfg)
}
