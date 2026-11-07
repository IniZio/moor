//! Re-runs the steps that never finished on a ready Box whose last start ended in setup-failed.

use moor_core::box_key::BoxKey;
use moor_core::config::BoxSettings;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::seam::engine::{AgentFate, EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::WorkspaceRef;
use moor_core::{BoxHandle, BoxPath, BoxTarget, CloseReq, ErrorKind};

use moor_core::config::Flags;
use moor_core::BoxId;

use super::interrupted::{launch_was_cut, release_warned};
use super::lease::planned_root;
use super::rerun::record_interrupted;
use super::{layers, recorded_workdir, reread, run_and_record, Entry, Provisioned, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::close;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::facts::settings_of;
use crate::ops::common::{cache, setup};
use crate::ops::start::launch_after_retry;
use crate::store::{BoxRow, Store};

/// Longest one question to the Box about its agent may take.
const FATE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

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

/// Picks up the start a controller restart cut off in Box `id`, which this process claimed, and
/// launches its agent as the start would have. An acquire runs again. Each setup script that ran
/// or is running is attached to rather than run again. The Box lock is held meanwhile; a Box that
/// is no longer claimed is left alone.
pub(crate) fn resume_setup(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let first = super::reread(store, id)?;
    if matches!(first.intent, Some(Intent::Closing { .. })) {
        let req = CloseReq {
            target: BoxTarget::Name(first.name),
            force: true,
        };
        return close::run(cx, &req).map(drop);
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?
        .interruptible(cx.env().cancel());
    let row = super::reread(store, id)?;
    let me = std::process::id();
    if row.intent.is_none() && launch_was_cut(store, &row)? {
        return finish_launch(cx, store, &row);
    }
    let acquiring = row.intent == Some(Intent::Provisioning { pid: me });
    if acquiring && row.lifecycle != BoxLifecycle::SettingUp {
        return release_cut(cx, store, &row);
    }
    if row.lifecycle != BoxLifecycle::SettingUp {
        return Ok(());
    }
    let done = if acquiring {
        let cfg = settings_of(cx, &row, &Flags::default())?;
        let goes_on = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Starting);
        let entry = if goes_on { Entry::Adopt } else { Entry::Again };
        super::provision(cx, store, id, entry, &cfg)?
    } else if row.intent == Some(Intent::Setup { pid: me }) {
        follow_setup(cx, store, &row)?
    } else {
        return Ok(());
    };
    if done.failed_step().is_none() {
        launch_after_retry(cx, store, id)?;
    }
    Ok(())
}

/// Ends the launch a restart cut off in `row`. An agent already in the Box is attached to, as a
/// second launch would replace it and start its work over. A Box that cannot say launches nothing,
/// and the next process asks again.
fn finish_launch(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let (Some((handle, workdir)), Some(ws)) = (machine(row), row.workspace_id.as_deref()) else {
        return Ok(());
    };
    setup::await_reachable(cx, handle)?;
    let session = SessionRef {
        id: BoxKey::new(row.id).session_ref(),
        handle: handle.clone(),
        workdir,
        pane: None,
        holder: cx.engine().new_holder(),
    };
    let unknown = |e: EngineError| CommandError::new(ErrorKind::Failed, e.to_string());
    match cx
        .engine()
        .agent_fate(&session, Deadline(FATE_WAIT))
        .map_err(unknown)?
    {
        AgentFate::Running => {
            let ws = WorkspaceRef::new(ws);
            let pane = cx
                .engine()
                .reattach(&session, &ws, &row.name)
                .map_err(unknown)?;
            store.begin_session(row.id, &session.id, Some(pane.as_str()), session.holder)?;
            Ok(())
        }
        AgentFate::Ended { .. } => launch_after_retry(cx, store, row.id).map(drop),
    }
}

/// Stops the lease a working Box's cut-off acquire left going on its host; this can wait for that
/// acquire, so it runs here and not before the controller serves.
fn release_cut(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if let Some(handle) = planned_root(store, row.id)? {
        release_warned(cx, store, row, &handle);
    }
    store.apply(Decision::new(row.id).intent(IntentChange::Clear))?;
    Ok(())
}

/// Follows the setup that `row`'s earlier process left, once the Box answers. A failure that is
/// not a drain's records the setup as interrupted.
fn follow_setup(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Provisioned, CommandError> {
    let cfg = settings_of(cx, row, &Flags::default())?;
    if let Some((handle, _)) = machine(row) {
        setup::await_reachable(cx, handle)?;
    }
    match set_up_in_machine(cx, store, row, &cfg, false) {
        Ok(done) => Ok(done),
        // The intent stays, so the next process claims this setup again.
        Err(e) if cx.env().draining().is_cancelled() => Err(e),
        Err(e) => {
            record_interrupted(cx, store, &reread(store, row.id)?)?;
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
