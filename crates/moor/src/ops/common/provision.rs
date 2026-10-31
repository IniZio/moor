//! Box provisioning shared by the Box ops: acquire a machine for a new Box, or again for a lost one.

use std::path::Path;

use moor_core::config::{BoxSettings, Flags};

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{
    BoxRepo, Decision, ExtraEvent, Field, Fields, Intent, IntentChange,
};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError, Released};
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
use crate::ops::common::view::box_view;
use crate::ops::common::{agent_install, profile_fetch};
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, Store};

use self::acquired::record_acquired;
use self::clone::{clone_with_proxy, SetupProxy};
use self::delivered::record_delivered;
pub(crate) use self::lease::{hold_root, hold_workdir, planned_root};
use self::outcome::dotfiles_warned;
pub(crate) use self::outcome::Provisioned;
use self::progress::progress;
use self::rerun::note_setup_not_rerun;
use self::retry::{kept_box_retry, retry_in_box};

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
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, op, boxlock::WAIT)?;
    let row = reread(store, row.id)?;
    let entry = entry_for(&row);
    if kept_box_retry(&row) {
        let cfg = settings_of(cx, &row, &Flags::default())?;
        return retry_in_box(cx, store, &row, &cfg).map(Some);
    }
    let Some(entry) = entry else {
        if ready {
            note_setup_not_rerun(cx, store, &row)?;
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
    match cx.driver().acquire_noting(&req, &mut progress(store, id)) {
        Ok(got) => {
            hold_root(store, id, root, &planned, &got)?;
            record_acquired(store, id, &got)?;
            hold_workdir(store, id, &got)?;
            let log_dir = setup::log_dir(cx.state_dir()?, id);
            let target =
                SetupTarget::new(&got, req.provider.as_str(), started.repo.as_ref(), &log_dir);
            run_and_record(cx, store, id, entry.begin().is_some(), &target, cfg)
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

/// Where setup runs: the freshly acquired Box, the repo it clones and where its setup log is kept.
struct SetupTarget<'a> {
    /// The Box setup runs in.
    handle: &'a BoxHandle,
    /// The provider that holds the Box.
    provider: &'a str,
    /// Working directory in the Box.
    workdir: &'a BoxPath,
    /// The repo the Box clones, with the author it commits as; `None` for an empty workdir.
    repo: Option<&'a BoxRepo>,
    /// Host directory for the Box's setup log.
    log_dir: &'a Path,
}

impl<'a> SetupTarget<'a> {
    /// The target for a freshly acquired Box.
    const fn new(
        got: &'a Acquired,
        provider: &'a str,
        repo: Option<&'a BoxRepo>,
        log_dir: &'a Path,
    ) -> Self {
        Self {
            handle: &got.handle,
            provider,
            workdir: &got.workdir,
            repo,
            log_dir,
        }
    }
}

/// Runs the setup lines through the Proxy; a Proxy that will not start fails the setup.
fn run_through_proxy(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
) -> setup::SetupRun {
    match proxy {
        Ok(proxy) => setup::run_setup(
            cx,
            target.handle,
            target.provider,
            target.workdir,
            target.log_dir,
            cfg,
            proxy.as_ref(),
        ),
        // Dotfiles report their own Proxy failure; no setup line was meant to run.
        Err(_) if cfg.setup.is_empty() => setup::SetupRun::Skipped,
        Err(reason) => {
            let name = store.box_row(id).ok().flatten().map(|r| r.name);
            setup::proxy_failed(cx, name.as_ref().map_or("<box>", |n| n.as_str()), reason)
        }
    }
}

/// Runs repo setup in a Box under the setup intent and records how it ended, clearing the intent.
///
/// `advance` moves the Box on to working; a Box already working gets a `setup_failed` event
/// instead.
fn run_and_record(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<Provisioned, CommandError> {
    let (cloned, proxy) = clone_with_proxy(cx, store, id, advance, target, cfg)?;
    if let Some(ms) = cloned.ms {
        store.apply(Decision::new(id).event(ExtraEvent::Cloned { ms }))?;
    }
    record_delivered(store, id, cloned.delivered)?;
    let run = run_through_proxy(cx, store, id, target, cfg, &proxy);
    let at = (target.handle, target.provider);
    let dotfiles = match dotfiles::apply(cx, store, at, id, cfg, &proxy) {
        Ok(d) => d,
        Err(f) => dotfiles_warned(cx, f),
    };
    if let Err(reason) = agent_install::apply(cx, store, at, id, cfg, &proxy) {
        cx.warn(agent_install::warning(&reason));
    }
    profile_fetch::apply(cx, store, at, id, cfg, &proxy)?;
    let mut done = Decision::new(id);
    if let DotfilesRun::Failed { tail } = &dotfiles {
        done = done.event(ExtraEvent::SetupFailed {
            line: format!("dotfiles: {tail}"),
        });
    }
    if advance {
        done = done
            .lifecycle(BoxLifecycleInput::Ready {
                setup: run.outcome(),
            })
            .session(SessionInput::Started);
    }
    if let Some(ms) = run.ms() {
        done = done.event(ExtraEvent::SetupFinished {
            ms,
            ok: !run.failed(),
        });
    }
    if !advance {
        if let SetupOutcome::Failed { line } = run.outcome() {
            done = done.event(ExtraEvent::SetupFailed { line });
        }
    }
    let failed_at = if run.failed() {
        Field::Set(cx.clock().now().as_millis())
    } else {
        Field::Clear
    };
    let done = done
        .set(Fields {
            setup_failed_at: failed_at,
            ..Fields::default()
        })
        .intent(IntentChange::Clear);
    store.apply(done)?;
    Ok(Provisioned {
        setup: run,
        dotfiles,
        work_tree: cloned.work_tree,
    })
}

/// Records a failed acquire; returns the error, carrying the Box row, to report.
fn failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    entry: Entry,
    req: &AcquireReq,
    err: DriverError,
) -> CommandError {
    let cause = err.to_string();
    let attempted: BoxHandle = cx.driver().planned_handle(req);
    let mut lost = Decision::new(id);
    lost.lifecycle_input = entry.begin().map(|_| BoxLifecycleInput::SetupFailed {
        cause: cause.clone(),
        which: None,
    });
    let lost = lost
        .box_input(BoxInput::AcquireFailed {
            cause,
            attempted: Some(attempted.clone()),
        })
        .intent(IntentChange::Clear);
    if let Err(write) = store.apply(lost) {
        return write.into();
    }
    if cx.driver().release(&attempted) == Ok(Released::AlreadyGone) {
        let gone = Decision::new(id).box_input(BoxInput::DriverMissing);
        if let Err(write) = store.apply(gone) {
            return write.into();
        }
    }
    let out = CommandError::from(err);
    match reread(store, id) {
        Ok(row) => out.with_value(serde_json::json!({ "box": box_view(&row, cx.config()) })),
        Err(_) => out,
    }
}

mod acquired;
mod clone;
mod delivered;
pub(crate) mod interrupted;
mod lease;
mod outcome;
mod progress;
mod rerun;
mod retry;
#[cfg(test)]
mod tests;
