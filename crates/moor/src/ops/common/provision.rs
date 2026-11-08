//! Box provisioning shared by the Box ops: acquire a machine for a new Box, or again for a lost one.

use moor_core::config::{BoxSettings, Flags};

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::seam::rng::Rng;
use moor_core::{BoxId, BoxPath, ErrorKind, ProviderId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::cache;
use crate::ops::common::facts::{provider_caps, require_reverse_tunnel, settings_of};
use crate::ops::common::guest;
use crate::ops::common::node::{self, Bound};
use crate::ops::common::setup;
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, Store};

use self::acquired::record_acquired;
use self::after_lines::steps_after_lines;
use self::clone::clone_with_proxy;
use self::delivered::record_delivered;
pub(crate) use self::entry::Entry;
use self::failed::{failed, forced_out};
pub(crate) use self::lease::{hold_root, hold_workdir, planned_root};
use self::others::set_up_all;
pub(crate) use self::outcome::Provisioned;
use self::outcome::{dotfiles_noted, setup_ended, step_outcome};
use self::progress::{acquire_timed, adopt_timed, goes_on_after};
use self::rerun::start_ready;
pub(crate) use self::retry::resume_setup;
use self::retry::{kept_box_retry, retry_in_box};
use self::target::SetupTarget;

/// Re-reads a Box row that must exist.
///
/// # Errors
/// The row is gone.
pub(crate) fn reread(store: &Store, id: BoxId) -> Result<BoxRow, CommandError> {
    store
        .box_row(id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Internal, "started Box vanished"))
}

/// Gives a Box with no machine, a failed setup in a lost machine or a lost machine a machine again.
///
/// Returns what ran, or `None` when nothing was provisioned. The Box lock is held while
/// provisioning and the row is read again under it; another op holding it is
/// refused as busy.
///
/// Settings come from the `.moor.yaml` stored when the Box started. Nothing is copied into a
/// ready Box, and its setup never runs again; a failed or interrupted setup is only warned about.
pub(crate) fn reprovision(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    op: &str,
) -> Result<Option<Provisioned>, CommandError> {
    let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
    if entry_for(row).is_none() && !kept_box_retry(row) && !ready {
        return Ok(None);
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, op, boxlock::WAIT)?
        .interruptible(cx.env().cancel());
    let row = reread(store, row.id)?;
    let entry = entry_for(&row);
    if kept_box_retry(&row) {
        let cfg = settings_of(cx, &row, &Flags::default())?;
        return retry_in_box(cx, store, &row, &cfg).map(Some);
    }
    let Some(entry) = entry else {
        if ready {
            start_ready(cx, store, &row)?;
        }
        return Ok(None);
    };
    let cfg = settings_of(cx, &row, &Flags::default())?;
    provision(cx, store, row.id, entry, &cfg).map(Some)
}

/// Rebuilds the machine path from the workdir text stored on the Box row.
pub(crate) fn recorded_workdir(stored: &str) -> Option<BoxPath> {
    BoxPath::restore(stored).ok()
}

/// How a Box that wants a machine starts over; `None` when it has one or is being provisioned.
fn entry_for(row: &BoxRow) -> Option<Entry> {
    let box_state = row.boxed.as_ref().map(|b| b.state);
    match (row.lifecycle, box_state) {
        (BoxLifecycle::Checking, None) if row.intent.is_none() => Some(Entry::Fresh),
        (BoxLifecycle::Pending, None) if row.intent.is_none() => Some(Entry::Waiting),
        (BoxLifecycle::SetupFailed, Some(BoxState::Failed | BoxState::Gone)) => Some(Entry::Retry),
        (BoxLifecycle::Ready, Some(BoxState::Failed | BoxState::Gone)) => Some(Entry::Stay),
        _ => None,
    }
}

/// What marks Box `id` as wanting a machine from `provider`. A takeover only claims the acquire
/// already under way.
fn requested(id: BoxId, entry: Entry, provider: &ProviderId) -> Decision {
    let claim = IntentChange::Set(Intent::Provisioning {
        pid: std::process::id(),
    });
    if entry == Entry::Adopt {
        return Decision::new(id).intent(claim);
    }
    let mut d = Decision::new(id);
    d.lifecycle_input = entry.begin();
    d.box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(provider.clone()),
            box_handle: Field::Clear,
            state_reason: entry.reason_once_admitted(),
            ..Fields::default()
        })
        .intent(claim)
}

/// Admits the Box, acquires its machine and records the outcome.
///
/// A new or waiting Box at the machine limit is queued (or left waiting) and the
/// driver is not called. The caller holds the Box lock.
pub(crate) fn provision(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    entry: Entry,
    cfg: &BoxSettings,
) -> Result<Provisioned, CommandError> {
    let started = reread(store, id)?;
    let requested = |provider: &ProviderId| requested(id, entry, provider);
    let bound = if matches!(entry, Entry::Fresh | Entry::Waiting) {
        let fresh = (&started, entry == Entry::Fresh);
        match node::admit(cx, store, fresh, requested)? {
            Some(bound) => bound,
            None => return Ok(Provisioned::skipped()),
        }
    } else {
        let provider = match started.boxed.as_ref() {
            Some(held) => held.provider.clone(),
            None => node::runtime_of(cx, store, &started)?,
        };
        let caps =
            require_reverse_tunnel(provider.as_str(), provider_caps(cx, provider.as_str())?)?;
        store.apply(requested(&provider))?;
        Bound {
            seat: node::seat_of(cx, store, &started)?,
            provider,
            caps,
        }
    };
    let req = acquire_req(cx, store, &started, cfg, bound);
    // The record exists before the Box does; a failed acquire leaves it `planned`.
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(id, LeaseKind::BoxRoot, planned.as_str())?;
    let provider = req.provider.as_str();
    let mut layered = layers::plan(cx, store, &started, provider, cfg, &req.cache);
    let acquired = machine_for(cx, store, id, entry, &req, layered.as_mut())?;
    let forced = || cx.env().cancel().is_cancelled();
    match acquired {
        // Whatever it made is the forced close's to release.
        _ if forced() => Err(forced_out(store, id, entry, &planned)?),
        Ok(got) => {
            hold_root(store, id, root, &planned, &got)?;
            record_acquired(store, id, &got, (entry == Entry::Stay).then_some(&started))?;
            hold_workdir(store, id, &got)?;
            let log_dir = setup::log_dir(cx.state_dir()?, id);
            let repo = started.repo.as_ref();
            let target = SetupTarget::new(&got, provider, repo, &log_dir, layered.as_ref());
            let done = run_and_record(cx, store, id, entry.sets_up(), &target, cfg)?;
            // A Box a forced close is waiting on gets no agent.
            if cx.env().cancel().is_cancelled() {
                return Err(CommandError::new(ErrorKind::Failed, setup::FORCED_CLOSE));
            }
            Ok(done)
        }
        // The next process takes it over, as the intent stays.
        Err(e) if cx.env().draining().is_cancelled() && goes_on_after(cx, &req, &e) => {
            Err(setup::Released.into())
        }
        Err(e) => Err(failed(cx, store, id, entry, &req, e)),
    }
}

/// Gets Box `id` its machine for `req`: from a kept layer, else by the acquire `entry` calls for. A
/// forced close already asked for gets it none.
fn machine_for(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    entry: Entry,
    req: &AcquireReq,
    layered: Option<&mut layers::Layered>,
) -> Result<Result<Acquired, DriverError>, CommandError> {
    let forced = || cx.env().cancel().is_cancelled();
    let warm = match layered {
        Some(layered) if entry != Entry::Adopt && !forced() => {
            layers::warm(cx, store, id, req, layered)?
        }
        _ => None,
    };
    let acquire = if entry == Entry::Adopt {
        adopt_timed
    } else {
        acquire_timed
    };
    Ok(warm.unwrap_or_else(|| {
        if forced() {
            return Err(DriverError::Failed {
                summary: setup::FORCED_CLOSE.to_owned(),
            });
        }
        acquire(cx, store, id, req)
    }))
}

/// What the driver is asked for `started` on `provider`.
fn acquire_req(
    cx: &Cx,
    store: &Store,
    started: &BoxRow,
    cfg: &BoxSettings,
    bound: Bound,
) -> AcquireReq {
    let Bound {
        provider,
        caps,
        seat,
        ..
    } = bound;
    AcquireReq {
        pool_machine: seat,
        cache: cache::for_start(cx, store, started, cfg, &provider, caps),
        provider,
        box_id: started.id,
        name: started.name.clone(),
        class: cfg.size.as_ref().map(|s| s.value.as_str().to_owned()),
        nonce: nonce(cx.rng()),
    }
}

/// Six lowercase hex chars drawn from the RNG, so each attempt differs.
pub(crate) fn nonce(rng: &dyn Rng) -> String {
    format!("{:06x}", rng.u32() & 0xFF_FFFF)
}

/// Runs repo setup in a Box under the setup intent and records how it ended, clearing the intent.
///
/// `advance` moves the Box on to working; a failed line or agent install moves any Box to
/// setup-failed, one that was already working included. The machine may have just come up, so
/// nothing touches it before its guest answers.
fn run_and_record(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<Provisioned, CommandError> {
    guest::await_guest(cx, store, id, target.handle)?;
    let (cloned, proxy) = clone_with_proxy(cx, store, id, advance, target, cfg)?;
    let on_fork = target;
    let (target, reran, stale) = &warm_rerun::after_clone(target, &cloned);
    let (target, reran, stale) = (target, *reran, *stale);
    layers::after_clone(cx, store, target, cloned.ms);
    if let Some(ms) = cloned.ms {
        store.apply(Decision::new(id).event(ExtraEvent::Cloned { ms }))?;
    }
    record_delivered(cx, store, id, cfg, cloned.delivered)?;
    let warm_disk = warm_rerun::record_disk(store, id, (stale, reran.is_some()), &cloned)?;
    let run = set_up_all(cx, store, id, target, cfg, &proxy)?;
    let retries = run.failed() && reran.is_some() && !cx.env().cancel().is_cancelled();
    if let Some(layered) = on_fork.layered.filter(|_| retries) {
        return warm_rerun::cold_again(cx, store, id, advance, on_fork, layered, cfg, &run);
    }
    if !run.failed() {
        store.apply(Decision::new(id).set(Fields {
            setup_failed_at: Field::Clear,
            steps_pending_at: Field::Set(cx.clock().now().as_millis()),
            ..Fields::default()
        }))?;
    }
    let (dotfiles, installed) = steps_after_lines(
        cx,
        store,
        id,
        (on_fork, target, cloned.at_base),
        (cfg, &proxy),
        &run,
    )?;
    let mut done = dotfiles_noted(id, &dotfiles);
    if let Some(ms) = run.ms() {
        done = done.event(ExtraEvent::SetupFinished {
            ms,
            ok: !run.failed(),
        });
    }
    let install_failed = installed.is_err();
    let outcome = step_outcome(&run, installed);
    let failed_at = stamped_when_failed(cx, &outcome);
    let done = setup_ended(done, advance, outcome)
        .setup_why(why::setup_why(store, id, &run))
        .set(Fields {
            setup_failed_at: failed_at,
            steps_pending_at: Field::Clear,
            ..Fields::default()
        })
        .intent(IntentChange::Clear);
    store.apply(done)?;
    Ok(Provisioned {
        setup: run,
        dotfiles,
        work_tree: cloned.work_tree,
        install_failed,
        warm_disk,
    })
}

/// `setup_failed_at` for a setup that ended in `outcome`: now when it failed, else cleared.
fn stamped_when_failed(cx: &Cx, outcome: &SetupOutcome) -> Field<i64> {
    match outcome {
        SetupOutcome::Failed { .. } => Field::Set(cx.clock().now().as_millis()),
        SetupOutcome::Ok => Field::Clear,
    }
}

mod acquired;
mod after_lines;
mod clone;
mod delivered;
mod disk;
mod entry;
pub(crate) use disk::relieve_disk;
mod failed;
pub(crate) mod interrupted;
mod layers;
mod lease;
mod others;
mod outcome;
mod progress;
mod rerun;
mod retry;
mod setup_run;
mod target;
mod warm_rerun;
mod why;
pub(crate) use warm_rerun::warm_disk_note;
#[cfg(test)]
mod tests;
