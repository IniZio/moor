//! Re-runs the steps that never finished on a ready Box whose last start ended in setup-failed.

use moor_core::activity::{Life, Probe, Term};
use moor_core::box_key::BoxKey;
use moor_core::config::BoxSettings;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Effect, EffectChange, Wanted, WantedKind};
use moor_core::domain::hands::HandsState;
use moor_core::seam::engine::{LaunchPrompt, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, BoxTarget, CloseReq, ErrorKind};

use moor_core::config::Flags;
use moor_core::BoxId;

use super::interrupted::{launch_was_cut, release_warned};
use super::lease::planned_root;
use super::rerun::{finish_steps, record_interrupted};
use super::{layers, recorded_workdir, reread, run_and_record, Entry, Provisioned, SetupTarget};
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::close;
use crate::ops::common::facts::settings_of;
use crate::ops::common::{cache, setup};
use crate::ops::start::{launch_after_retry, queued_or};
use crate::store::{BoxRow, Store};

/// Longest one question to the Box about its agent may take.
const FATE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The handle and workdir of the machine `row` holds, when it is ready.
fn machine(row: &BoxRow) -> Option<(&BoxHandle, BoxPath)> {
    let boxed = row
        .hands
        .as_ref()
        .filter(|b| b.state == HandsState::Ready)?;
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

/// Runs the clone, setup and dotfiles in the kept Box as the setup effect, then marks it ready.
///
/// The caller holds the Box's turn and passes the row read under it. No Box is acquired. The
/// effect ends on success and with the failure record, so a crash mid-run leaves it for the next
/// process to settle.
pub(super) fn retry_in_box(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
) -> Result<Provisioned, CommandError> {
    let begin = Decision::new(row.id)
        .lifecycle(BoxLifecycleInput::Retry)
        .effect(EffectChange::Next(Effect::Setup));
    store.apply(begin)?;
    set_up_in_machine(cx, store, row, cfg, Fork::New)
}

/// Picks up the start a controller restart cut off in Box `id`, which [`super::interrupted::settle`]
/// let go on, and launches its agent as the start would have. An acquire runs again. Each setup
/// script that ran or is running is attached to rather than run again. A wanted close, or a
/// release it cut, is finished. All of it runs under the Box's Resume turn, the caller's when it
/// holds one.
pub(crate) fn resume_setup(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let _turn = cx.turn_unless_held(id, Cmd::Resume)?;
    layers::release_left(cx, store, id)?;
    let row = super::reread(store, id)?;
    let wanted = row
        .wanted
        .as_ref()
        .filter(|w| w.kind() == WantedKind::Close);
    if wanted.is_some() || row.effect == Some(Effect::Release) {
        let req = CloseReq {
            target: BoxTarget::Name(row.name.clone()),
            force: wanted.is_none_or(Wanted::force),
        };
        let reason = wanted.and_then(Wanted::reason).map(str::to_owned);
        return close::run_with_reason(cx, &req, reason).map(drop);
    }
    if launch_was_cut(store, &row)? {
        return finish_launch(cx, store, &row);
    }
    let acquiring = row.effect == Some(Effect::Acquire);
    if acquiring && row.lifecycle != BoxLifecycle::SettingUp {
        return release_cut(cx, store, &row);
    }
    if row.lifecycle != BoxLifecycle::SettingUp {
        return Ok(());
    }
    let done = if acquiring {
        let cfg = settings_of(cx, &row, &Flags::default())?;
        let goes_on = row.hands.as_ref().map(|b| b.state) == Some(HandsState::Starting);
        let entry = if goes_on { Entry::Adopt } else { Entry::Again };
        super::provision(cx, store, id, entry, &cfg)?
    } else if row.effect == Some(Effect::Setup) && row.steps_pending_at.is_some() {
        finish_cut_steps(cx, store, &row)?
    } else if row.effect == Some(Effect::Setup) {
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
/// and the launch stays in flight for the next process to ask again.
fn finish_launch(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    launch_or_attach(cx, store, row)?;
    store.apply(Decision::new(row.id).effect(EffectChange::End))?;
    Ok(())
}

/// Attaches to the agent the cut launch of `row` left running, or launches it.
fn launch_or_attach(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let Some((handle, workdir)) = machine(row) else {
        return Ok(());
    };
    setup::await_reachable(cx, handle)?;
    let session = SessionRef {
        id: BoxKey::new(row.id).session_ref(),
        handle: handle.clone(),
        workdir,
    };
    store.require_hc_holder(row.id, row.name.as_str())?;
    match cx.engine().probe(&session, Deadline(FATE_WAIT)) {
        Probe::Read {
            term: Some(Term {
                life: Life::Live, ..
            }),
            ..
        } => {
            // The launch a restart cut off carried the oldest queued prompt unless it resumed.
            let fresh = row.agent_session_id.as_deref().is_none_or(str::is_empty);
            let argv = if fresh {
                queued_or(store, row.id, LaunchPrompt::None)?
            } else {
                LaunchPrompt::None
            };
            store.begin_launch_with(row.id, &session.id, argv.text())?;
            Ok(())
        }
        Probe::Read { .. } => launch_after_retry(cx, store, row.id).map(drop),
        Probe::TimedOut => Err(CommandError::new(
            ErrorKind::Failed,
            "the Box did not say whether its agent runs",
        )),
    }
}

/// Stops the lease a working Box's cut-off acquire left going on its host; this can wait for that
/// acquire, so it runs here and not before the controller serves.
fn release_cut(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if let Some(handle) = planned_root(store, row.id)? {
        release_warned(cx, store, row, &handle);
    }
    store.apply(Decision::new(row.id).effect(EffectChange::End))?;
    Ok(())
}

/// Follows the setup that `row`'s earlier process left, once the Box answers. A failure that is
/// not a drain's records the setup as interrupted.
fn follow_setup(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Provisioned, CommandError> {
    let cfg = settings_of(cx, row, &Flags::default())?;
    if let Some((handle, _)) = machine(row) {
        setup::await_reachable(cx, handle)?;
    }
    let fork = cut_fork(cx, store, row)?;
    match set_up_in_machine(cx, store, row, &cfg, fork) {
        Ok(done) => Ok(done),
        // The effect stays, so the next process claims this setup again.
        Err(e) if cx.env().draining().is_cancelled() => Err(e),
        Err(e) => {
            let why = format!(
                "a restarted controller could not follow the setup: {}",
                e.error
            );
            record_interrupted(cx, store, &reread(store, row.id)?, why)?;
            Err(e)
        }
    }
}

/// Finishes the steps after the setup lines that `row`'s earlier process left. A failure that is
/// not a drain's records the setup as failed with its error.
fn finish_cut_steps(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Provisioned, CommandError> {
    match finish_steps(cx, store, row, true) {
        Ok(done) => Ok(done),
        // The effect stays, so the next process claims these steps again.
        Err(e) if cx.env().draining().is_cancelled() => Err(e),
        Err(e) => {
            let why = format!(
                "a restarted controller could not finish the setup steps: {}",
                e.error
            );
            record_interrupted(cx, store, &reread(store, row.id)?, why)?;
            Err(e)
        }
    }
}

/// What a retried setup does about a fork onto a checkpoint.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fork {
    /// Runs in the machine the Box holds.
    No,
    /// Forks the nearest checkpoint first.
    New,
    /// Takes over the fork an earlier process left going.
    Adopt,
}

/// What became of a retry fork an earlier process left: taken over when its host keeps it going,
/// else released, as nothing else would.
fn cut_fork(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Fork, CommandError> {
    let held = row.hands.as_ref().and_then(|b| b.handle.as_ref());
    let Some(planned) = planned_root(store, row.id)?.filter(|p| Some(p) != held) else {
        return Ok(Fork::No);
    };
    if cx.driver().acquire_outlives_caller(&planned) {
        return Ok(Fork::Adopt);
    }
    if release_warned(cx, store, row, &planned) {
        for lease in store.leases(row.id)? {
            if lease.key == planned.as_str() {
                store.drop_lease(lease.id)?;
            }
        }
    }
    Ok(Fork::No)
}

/// Runs the clone, setup and dotfiles in the machine `row` holds, then marks it ready; `fork`
/// first moves a retried Box onto the checkpoint nearest its failed line.
fn set_up_in_machine(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    fork: Fork,
) -> Result<Provisioned, CommandError> {
    let (Some((handle, workdir)), Some(held)) = (machine(row), row.hands.as_ref()) else {
        return Err(CommandError::new(ErrorKind::Internal, "kept Box vanished"));
    };
    let provider = held.provider.as_str();
    let plan = cache::derived(cx, store, row, cfg, &held.provider);
    let mut layered = layers::plan(cx, store, row, provider, cfg, &plan);
    let forked = match layered.as_mut() {
        Some(layered) if fork != Fork::No => {
            layers::stand_on_rerun(store, row.id, layered)?;
            let adopt = fork == Fork::Adopt;
            layers::resume(cx, store, row, cfg, (layered, adopt))?
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
    let inc = reread(store, row.id)?.hands_inc();
    run_and_record(cx, store, (row.id, inc), true, &target, cfg)
}
