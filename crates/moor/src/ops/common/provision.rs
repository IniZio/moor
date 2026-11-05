//! Box provisioning shared by the Box ops: acquire a machine for a new Box, or again for a lost one.

use moor_core::config::{BoxSettings, Flags};

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::AcquireReq;
use moor_core::seam::rng::Rng;
use moor_core::{BoxHandle, BoxId, BoxPath, ErrorKind, ProviderId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::cache;
use crate::ops::common::dotfiles::{self, DotfilesRun};
use crate::ops::common::facts::{provider_caps, require_reverse_tunnel, settings_of};
use crate::ops::common::node::{self, Bound};
use crate::ops::common::setup;
use crate::ops::common::{agent_install, profile_fetch};
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, Store};

use self::acquired::record_acquired;
use self::clone::clone_with_proxy;
use self::delivered::record_delivered;
use self::failed::failed;
pub(crate) use self::lease::{hold_root, hold_workdir, planned_root};
use self::others::set_up_all;
use self::outcome::dotfiles_warned;
pub(crate) use self::outcome::Provisioned;
use self::progress::acquire_timed;
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

/// Where the Box stands when its machine is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A new Box still being checked.
    Fresh,
    /// A Box whose setup failed.
    Retry,
    /// A working Box that lost its machine; its state stays.
    Stay,
    /// A Box queued until a machine slot frees.
    Waiting,
}

impl Entry {
    /// The Box input that starts the step, if any.
    const fn begin(self) -> Option<BoxLifecycleInput> {
        match self {
            Self::Fresh => Some(BoxLifecycleInput::Admitted),
            Self::Waiting => Some(BoxLifecycleInput::Promoted),
            Self::Retry => Some(BoxLifecycleInput::Retry),
            Self::Stay => None,
        }
    }
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
    let requested = |provider: &ProviderId| {
        let mut d = Decision::new(id);
        d.lifecycle_input = entry.begin();
        d.box_input(BoxInput::Requested)
            .set(Fields {
                box_provider: Field::Set(provider.clone()),
                box_handle: Field::Clear,
                ..Fields::default()
            })
            .intent(IntentChange::Set(Intent::Provisioning {
                pid: std::process::id(),
            }))
    };
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
    let mut layered = layers::plan(cx, &started, provider, cfg, &req.cache);
    let warm = match layered.as_mut() {
        Some(layered) => layers::warm(cx, store, id, &req, layered)?,
        None => None,
    };
    match warm.map_or_else(|| acquire_timed(cx, store, id, &req), Ok) {
        Ok(got) => {
            hold_root(store, id, root, &planned, &got)?;
            record_acquired(store, id, &got)?;
            hold_workdir(store, id, &got)?;
            let log_dir = setup::log_dir(cx.state_dir()?, id);
            let repo = started.repo.as_ref();
            let target = SetupTarget::new(&got, provider, repo, &log_dir, layered.as_ref());
            let done = run_and_record(cx, store, id, entry.begin().is_some(), &target, cfg)?;
            // A Box a forced close is waiting on gets no agent.
            if cx.env().cancel().is_cancelled() {
                return Err(CommandError::new(ErrorKind::Failed, setup::FORCED_CLOSE));
            }
            Ok(done)
        }
        Err(e) => Err(failed(cx, store, id, entry, &req, e)),
    }
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
        class: Some(cfg.size.value.as_str().to_owned()),
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
/// setup-failed, one that was already working included.
fn run_and_record(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<Provisioned, CommandError> {
    let (cloned, proxy) = clone_with_proxy(cx, store, id, advance, target, cfg)?;
    // Checkpoints are keyed by the base, so a tree that is not the base unchanged saves none.
    let target = &target.keeping_layers(cloned.at_base);
    layers::after_clone(cx, store, target, cloned.ms);
    if let Some(ms) = cloned.ms {
        store.apply(Decision::new(id).event(ExtraEvent::Cloned { ms }))?;
    }
    record_delivered(cx, store, id, cfg.size.value, cloned.delivered)?;
    let run = set_up_all(cx, store, id, target, cfg, &proxy);
    if !run.failed() {
        store.apply(Decision::new(id).set(Fields {
            setup_failed_at: Field::Clear,
            steps_pending_at: Field::Set(cx.clock().now().as_millis()),
            ..Fields::default()
        }))?;
    }
    // A Box failing a line gets no agent, so the steps that prepare one wait for the retry.
    let (dotfiles, installed) = if run.failed() {
        (DotfilesRun::Skipped, Ok(()))
    } else {
        after_setup(cx, store, (target.handle, target.provider), id, cfg, &proxy)?
    };
    let mut done = Decision::new(id);
    if let DotfilesRun::Failed { tail } = &dotfiles {
        done = done.event(ExtraEvent::SetupFailed {
            line: format!("dotfiles: {tail}"),
        });
    }
    if let Some(ms) = run.ms() {
        done = done.event(ExtraEvent::SetupFinished {
            ms,
            ok: !run.failed(),
        });
    }
    let install_failed = installed.is_err();
    let outcome = match (run.outcome(), installed) {
        (SetupOutcome::Ok, Err(cause)) => SetupOutcome::Failed { line: cause },
        (outcome, _) => outcome,
    };
    let failed_at = if matches!(outcome, SetupOutcome::Failed { .. }) {
        Field::Set(cx.clock().now().as_millis())
    } else {
        Field::Clear
    };
    let done = setup_ended(done, advance, outcome)
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
    })
}

/// Adds how setup ended to `done`: `advance` moves the Box to working, and a failed step moves it
/// to setup-failed, from working too.
fn setup_ended(done: Decision, advance: bool, outcome: SetupOutcome) -> Decision {
    match (advance, outcome) {
        (true, SetupOutcome::Ok) => done
            .lifecycle(BoxLifecycleInput::Ready {
                setup: SetupOutcome::Ok,
            })
            .session(SessionInput::Started),
        (_, SetupOutcome::Failed { line }) => step_failed(done, advance, line),
        (false, SetupOutcome::Ok) => done,
    }
}

/// Moves the Box to setup-failed for a failed step of coming up, `cause` naming the step and its
/// error; `advance` says the Box was being set up rather than already working.
pub(super) fn step_failed(done: Decision, advance: bool, cause: String) -> Decision {
    if advance {
        return done.lifecycle(BoxLifecycleInput::SetupFailed { cause, which: None });
    }
    // The working Box's agent has no machine or no install, so its Session ends and a retry
    // starts a new one.
    done.lifecycle(BoxLifecycleInput::WorkingSetupFailed { cause })
        .session(SessionInput::End)
}

/// Runs the steps after the setup lines. A failed agent install is warned and returned as the
/// Box's failed step, and the profile, which needs the agent, waits for the retry.
pub(super) fn after_setup(
    cx: &Cx,
    store: &Store,
    at: (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &clone::SetupProxy,
) -> Result<(DotfilesRun, Result<(), String>), CommandError> {
    let dotfiles = match dotfiles::apply(cx, store, at, id, cfg, proxy) {
        Ok(d) => d,
        Err(f) => dotfiles_warned(cx, f),
    };
    if let Err(reason) = agent_install::apply(cx, store, at, id, cfg, proxy) {
        cx.warn(agent_install::warning(&reason));
        return Ok((dotfiles, Err(format!("agent install: {reason}"))));
    }
    profile_fetch::apply(cx, store, at, id, cfg, proxy)?;
    Ok((dotfiles, Ok(())))
}

mod acquired;
mod clone;
mod delivered;
mod disk;
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
#[cfg(test)]
mod tests;
