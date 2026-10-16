//! Box provisioning shared by the task ops: acquire for a new Task, or again for a lost Box.

use std::path::{Path, PathBuf};

use moor_core::config::{Flags, TaskSettings};

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError, Released};
use moor_core::seam::rng::Rng;
use moor_core::{BoxHandle, BoxPath, ErrorKind, ProviderId, TaskId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::capacity;
use crate::ops::common::dotfiles;
use crate::ops::common::facts::{provider_caps, require_reverse_tunnel, resolved};
use crate::ops::common::identity;
use crate::ops::common::setup;
use crate::ops::common::view::task_view;
use crate::store::{Store, TaskRow};

pub(crate) use self::outcome::Provisioned;
use self::outcome::{dotfiles_failed, sync_failed};
pub(crate) use self::rerun::ResyncGates;
use self::rerun::{note_setup_not_rerun, resync};
use self::retry::{dotfiles_retry, retry_dotfiles};

/// Re-reads a Task row that must exist.
///
/// # Errors
/// The row is gone.
pub(crate) fn reread(store: &Store, id: TaskId) -> Result<TaskRow, CommandError> {
    store
        .task(id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Internal, "started task vanished"))
}

/// Where the Task stands when its Box is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A new Task still being checked.
    Fresh,
    /// A Task whose setup failed.
    Retry,
    /// A working Task that lost its Box; its state stays.
    Stay,
    /// A Task queued until a Box slot frees.
    Waiting,
}

impl Entry {
    /// The Task input that starts the step, if any.
    const fn begin(self) -> Option<TaskInput> {
        match self {
            Self::Fresh => Some(TaskInput::Admitted),
            Self::Waiting => Some(TaskInput::Promoted),
            Self::Retry => Some(TaskInput::Retry),
            Self::Stay => None,
        }
    }
}

/// Gives a Task with no Box, a failed setup in a lost Box or a lost Box a Box again.
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
    row: &TaskRow,
    op: &str,
    trusted: bool,
    gates: &ResyncGates,
) -> Result<Option<Provisioned>, CommandError> {
    let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
    if entry_for(row).is_none() && !dotfiles_retry(row) && !ready {
        return Ok(None);
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, op, boxlock::WAIT)?;
    let row = reread(store, row.id)?;
    let entry = entry_for(&row);
    if dotfiles_retry(&row) {
        return retry_dotfiles(cx, store, &row).map(Some);
    }
    let Some(entry) = entry else {
        if ready {
            if row.intent.is_none() {
                resync(cx, store, &row, trusted, gates)?;
            }
            note_setup_not_rerun(cx, store, &row)?;
        }
        return Ok(None);
    };
    let repo = moor_yaml::guard(cx, &row, trusted)?;
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
    provision(cx, store, row.id, entry, &cfg).map(Some)
}

/// Rebuilds the Box path from the workdir text stored on the Task row.
pub(crate) fn recorded_workdir(stored: &str) -> Option<BoxPath> {
    BoxPath::restore(stored).ok()
}

/// How a Task that wants a Box starts over; `None` when it has one or is being provisioned.
fn entry_for(row: &TaskRow) -> Option<Entry> {
    let box_state = row.boxed.as_ref().map(|b| b.state);
    match (row.state, box_state) {
        (TaskState::Checking, None) if row.intent.is_none() => Some(Entry::Fresh),
        (TaskState::WaitingForBox, None) if row.intent.is_none() => Some(Entry::Waiting),
        (TaskState::SetupFailed, Some(BoxState::Failed | BoxState::Gone)) => Some(Entry::Retry),
        (TaskState::Working | TaskState::Idle, Some(BoxState::Failed | BoxState::Gone)) => {
            Some(Entry::Stay)
        }
        _ => None,
    }
}

/// Admits the Task, acquires its Box and records the outcome.
///
/// A new or waiting Task at the Box limit is queued (or left waiting) and the
/// driver is not called. The caller holds the Box lock.
pub(crate) fn provision(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    entry: Entry,
    cfg: &TaskSettings,
) -> Result<Provisioned, CommandError> {
    let task = reread(store, id)?;
    let provider = ProviderId::new(cfg.provider.value.clone());
    require_reverse_tunnel(cfg, provider_caps(cx, cfg)?)?;
    let mut requested = Decision::new(id);
    requested.task_input = entry.begin();
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
                let queued = TaskInput::AtCapacity {
                    max_boxes: lim.max.get(),
                };
                Decision::new(id).task_input(queued)
            })
        })?;
        if full {
            return Ok(Provisioned::skipped());
        }
    } else {
        store.apply(requested)?;
    }
    let worktree = PathBuf::from(task.worktree.as_deref().unwrap_or(&task.repo));
    let req = AcquireReq {
        provider,
        task: id,
        name: task.name,
        class: Some(cfg.size.value.as_str().to_owned()),
        nonce: nonce(cx.rng()),
        worktree: task.worktree.as_deref().map(PathBuf::from),
    };
    match cx.driver().acquire(&req) {
        Ok(got) => {
            record_acquired(store, id, &got)?;
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
fn record_acquired(store: &Store, id: TaskId, got: &Acquired) -> Result<(), CommandError> {
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

/// Where setup runs: the Box, and the worktree whose author the Box commits as.
struct SetupTarget<'a> {
    /// The Box setup runs in.
    handle: &'a BoxHandle,
    /// Working directory in the Box.
    workdir: &'a BoxPath,
    /// Worktree on the host the Task works in.
    worktree: &'a Path,
    /// How the worktree is synced into the Box before setup runs.
    sync: SyncMode,
    /// Whether an agent launches after setup; `None` when it always does.
    launching: Option<&'a dyn Fn() -> Result<bool, CommandError>>,
}

impl<'a> SetupTarget<'a> {
    /// The target for a freshly acquired Box.
    const fn new(got: &'a Acquired, worktree: &'a Path) -> Self {
        Self {
            handle: &got.handle,
            workdir: &got.workdir,
            worktree,
            sync: SyncMode::Full,
            launching: None,
        }
    }
}

/// Syncs the checkout into the Box when the target asks for it. A failure aborts when `advance`
/// or when an agent launches after setup; otherwise it is a warning and setup carries on without
/// the sync.
fn sync_into_box(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &TaskSettings,
) -> Result<(), CommandError> {
    if target.sync != SyncMode::Full {
        return Ok(());
    }
    let Err(reason) = setup::sync_checkout(cx, target.handle, target.workdir, target.worktree, cfg)
    else {
        return Ok(());
    };
    let launching = target.launching.map_or(Ok(true), |f| f())?;
    if advance || launching {
        return Err(sync_failed(cx, store, id, advance, reason));
    }
    cx.warn(Warning {
        code: "resync_failed".to_owned(),
        message: format!("{reason}; run `moor start` again"),
    });
    Ok(())
}

/// Runs repo setup in a Box under the setup intent and records how it ended, clearing the intent.
///
/// `advance` moves the Task on to working; a Task already working gets a `setup_failed` event
/// instead.
fn run_and_record(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &TaskSettings,
) -> Result<Provisioned, CommandError> {
    identity::set_identity(cx, target.handle, target.worktree);
    sync_into_box(cx, store, id, advance, target, cfg)?;
    // The checkout is in the Box by now, or was meant to stay as it is.
    let run = setup::run_setup(
        cx,
        target.handle,
        target.workdir,
        target.worktree,
        cfg,
        SyncMode::None,
    );
    let setup_line = match run.outcome() {
        SetupOutcome::Failed { line } => Some(line),
        SetupOutcome::Ok => None,
    };
    let dotfiles = match dotfiles::apply(cx, target.handle, id, target.worktree, cfg) {
        Ok(d) => d,
        Err(f) => return Err(dotfiles_failed(cx, store, id, advance, setup_line, f)),
    };
    let mut done = Decision::new(id);
    done.task_input = advance.then(|| TaskInput::BoxReady {
        setup: run.outcome(),
    });
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

/// Records a failed acquire; returns the error, carrying the Task row, to report.
fn failed(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    entry: Entry,
    req: &AcquireReq,
    err: DriverError,
) -> CommandError {
    let cause = err.to_string();
    let attempted: BoxHandle = cx.driver().planned_handle(req);
    let mut lost = Decision::new(id);
    lost.task_input = entry.begin().map(|_| TaskInput::AcquireFailed {
        cause: cause.clone(),
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
        Ok(row) => out.with_value(serde_json::json!({ "task": task_view(&row, cx.config()) })),
        Err(_) => out,
    }
}

pub(crate) mod moor_yaml;
mod outcome;
mod rerun;
mod retry;
#[cfg(test)]
mod tests;
