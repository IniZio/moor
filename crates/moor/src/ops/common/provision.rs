//! Box provisioning shared by the Box ops: acquire a machine for a new Box, or again for a lost one.

use std::path::{Path, PathBuf};

use moor_core::config::{BoxSettings, Flags};

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::domain::run::SyncMode;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError, Released};
use moor_core::seam::rng::Rng;
use moor_core::{BoxHandle, BoxId, BoxPath, ErrorKind, ProviderId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::capacity;
use crate::ops::common::dotfiles::{self, DotfilesRun};
use crate::ops::common::facts::{provider_caps, require_reverse_tunnel, resolved};
use crate::ops::common::identity;
use crate::ops::common::setup;
use crate::ops::common::view::box_view;
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, Store};

pub(crate) use self::lease::hold_root;
use self::lease::hold_workdir;
pub(crate) use self::outcome::Provisioned;
use self::outcome::{dotfiles_warned, sync_failed};
pub(crate) use self::rerun::ResyncGates;
use self::rerun::{note_setup_not_rerun, resync};
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
/// A `.moor.yaml` that differs from the base commit is refused unless `trusted`, before any
/// setup or Box work starts. `gates` decide a re-sync of the checkout into a ready Box. A ready
/// Box never has its setup run again; a failed or interrupted setup is only warned about.
pub(crate) fn reprovision(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    op: &str,
    trusted: bool,
    gates: &ResyncGates,
) -> Result<Option<Provisioned>, CommandError> {
    let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
    if entry_for(row).is_none() && !kept_box_retry(row) && !ready {
        return Ok(None);
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, op, boxlock::WAIT)?;
    let row = reread(store, row.id)?;
    let entry = entry_for(&row);
    if kept_box_retry(&row) {
        let repo = moor_yaml::guard(cx, &row, trusted)?;
        let cfg = resolved(cx, repo.as_ref(), &Flags::default());
        return retry_in_box(cx, store, &row, &cfg).map(Some);
    }
    let Some(entry) = entry else {
        if ready {
            note_setup_not_rerun(cx, store, &row)?;
            let row = reread(store, row.id)?;
            if row.intent.is_none() {
                resync(cx, store, &row, trusted, gates)?;
            }
        }
        return Ok(None);
    };
    let repo = moor_yaml::guard(cx, &row, trusted)?;
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
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
        (BoxLifecycle::WaitingForBox, None) if row.intent.is_none() => Some(Entry::Waiting),
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
    let provider = ProviderId::new(cfg.provider.value.clone());
    require_reverse_tunnel(cfg, provider_caps(cx, cfg)?)?;
    let mut requested = Decision::new(id);
    requested.lifecycle_input = entry.begin();
    let requested = requested
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(provider.clone()),
            box_handle: Field::Clear,
            ..Fields::default()
        })
        .intent(IntentChange::Set(Intent::Provisioning {
            pid: std::process::id(),
        }));
    if matches!(entry, Entry::Fresh | Entry::Waiting) {
        let mut full = false;
        store.apply_admitting(&provider, |holders| {
            let Some(lim) = capacity::full(cx.config(), &provider, holders) else {
                return Some(requested);
            };
            full = true;
            (entry == Entry::Fresh).then(|| {
                let queued = BoxLifecycleInput::AtCapacity {
                    max_boxes: lim.max.get(),
                };
                Decision::new(id).lifecycle(queued)
            })
        })?;
        if full {
            return Ok(Provisioned::skipped());
        }
    } else {
        store.apply(requested)?;
    }
    let worktree = PathBuf::from(started.worktree.as_deref().unwrap_or(&started.repo));
    let req = AcquireReq {
        provider,
        box_id: id,
        name: started.name,
        class: Some(cfg.size.value.as_str().to_owned()),
        nonce: nonce(cx.rng()),
        worktree: started.worktree.as_deref().map(PathBuf::from),
    };
    // The record exists before the Box does; a failed acquire leaves it `planned`.
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(id, LeaseKind::BoxRoot, planned.as_str())?;
    match cx.driver().acquire(&req) {
        Ok(got) => {
            hold_root(store, id, root, &planned, &got)?;
            record_acquired(store, id, &got)?;
            hold_workdir(store, id, &got)?;
            let target = SetupTarget::new(&got, &worktree);
            run_and_record(cx, store, id, entry.begin().is_some(), &target, cfg)
        }
        Err(e) => Err(failed(cx, store, id, entry, &req, e)),
    }
}

/// Six lowercase hex chars drawn from the RNG, so each attempt differs.
pub(crate) fn nonce(rng: &dyn Rng) -> String {
    format!("{:06x}", rng.u32() & 0xFF_FFFF)
}

/// Records an acquired Box under the setup intent.
fn record_acquired(store: &Store, id: BoxId, got: &Acquired) -> Result<(), CommandError> {
    let acquired = Decision::new(id)
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_handle: Field::Set(got.handle.clone()),
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            ..Fields::default()
        })
        .intent(IntentChange::Set(Intent::Setup {
            pid: std::process::id(),
        }));
    store.apply(acquired)?;
    Ok(())
}

/// Where setup runs: the freshly acquired Box, and the worktree whose author the Box commits as.
struct SetupTarget<'a> {
    /// The Box setup runs in.
    handle: &'a BoxHandle,
    /// Working directory in the Box.
    workdir: &'a BoxPath,
    /// Worktree on the host the Box works in.
    worktree: &'a Path,
}

impl<'a> SetupTarget<'a> {
    /// The target for a freshly acquired Box.
    const fn new(got: &'a Acquired, worktree: &'a Path) -> Self {
        Self {
            handle: &got.handle,
            workdir: &got.workdir,
            worktree,
        }
    }
}

/// Syncs the checkout into the Box. A failure aborts provisioning.
fn sync_into_box(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<(), CommandError> {
    setup::sync_checkout(cx, target.handle, target.workdir, target.worktree, cfg)
        .map_err(|reason| sync_failed(cx, store, id, advance, target.handle, reason))
}

/// Runs the setup lines through the Proxy; a Proxy that will not start fails the setup.
fn run_through_proxy(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> setup::SetupRun {
    let proxy = (!cfg.setup.is_empty())
        .then(|| box_exec::proxy(cx, id, Some(target.handle.clone()), target.worktree, cfg))
        .transpose();
    match proxy {
        Ok(proxy) => setup::run_setup(
            cx,
            target.handle,
            &cfg.provider.value,
            target.workdir,
            target.worktree,
            cfg,
            SyncMode::None,
            proxy.as_ref(),
        ),
        Err(reason) => {
            let name = store
                .box_row(id)
                .ok()
                .flatten()
                .map(|r| r.name.as_str().to_owned());
            setup::proxy_failed(cx, name.as_deref().unwrap_or("<box>"), &reason)
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
    identity::set_identity(cx, target.handle, target.worktree);
    sync_into_box(cx, store, id, advance, target, cfg)?;
    // The checkout is in the Box by now, or was meant to stay as it is.
    let run = run_through_proxy(cx, store, id, target, cfg);
    let dotfiles = match dotfiles::apply(cx, target.handle, id, target.worktree, cfg) {
        Ok(d) => d,
        Err(f) => dotfiles_warned(cx, f),
    };
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
    if cx.driver().release(&attempted, req.worktree.as_deref()) == Ok(Released::AlreadyGone) {
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

mod lease;
pub(crate) mod moor_yaml;
mod outcome;
mod rerun;
mod retry;
#[cfg(test)]
mod tests;
