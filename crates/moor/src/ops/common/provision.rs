//! Box provisioning shared by the task ops: acquire for a new Task, or again for a lost Box.

use std::path::{Path, PathBuf};

use moor_core::config::egress::{effective_allow_list, provider_applies_policy, unenforced_note};
use moor_core::config::{Flags, TaskSettings};
use moor_core::decide::intent::setup_dead;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{
    AcquireReq, Acquired, DriverError, EgressApplied, EgressPlan, Released,
};
use moor_core::seam::rng::Rng;
use moor_core::{BoxHandle, BoxPath, ErrorKind, ProviderId, TaskId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::capacity;
use crate::ops::common::dotfiles;
use crate::ops::common::facts::settings;
use crate::ops::common::identity;
use crate::ops::common::setup;
use crate::ops::common::view::task_view;
use crate::store::{Store, TaskRow};

use self::outcome::dotfiles_failed;
pub(crate) use self::outcome::Provisioned;
use self::retry::retry_dotfiles;

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

/// The egress plan the settings call for.
///
/// # Errors
/// A repo `egress` entry that is not a valid domain.
pub(crate) fn egress_plan(cfg: &TaskSettings) -> Result<EgressPlan, CommandError> {
    if provider_applies_policy(&cfg.provider.value, cfg.enforce_egress) {
        Ok(EgressPlan::Allow(effective_allow_list(
            &cfg.user_egress,
            &cfg.egress,
        )?))
    } else {
        Ok(EgressPlan::Open)
    }
}

/// Gives a Task with no Box, a failed setup or a lost Box a Box again.
///
/// Returns what ran, or `None` when nothing was provisioned. The Box lock is held while
/// provisioning and the row is read again under it; another op holding it is
/// refused as busy.
pub(crate) fn reprovision(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    op: &str,
) -> Result<Option<Provisioned>, CommandError> {
    if entry_for(row).is_none() && !setup_wanted(cx, row) && !dotfiles_retry(row) {
        return Ok(None);
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, op, boxlock::WAIT)?;
    let row = reread(store, row.id)?;
    if dotfiles_retry(&row) {
        return retry_dotfiles(cx, store, &row).map(Some);
    }
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    if let Some(entry) = entry_for(&row) {
        let cfg = settings(cx, Path::new(at), &Flags::default())?;
        return provision(cx, store, row.id, entry, &cfg).map(Some);
    }
    let (true, Some(handle), Some(workdir)) = (
        setup_wanted(cx, &row),
        row.boxed.as_ref().and_then(|b| b.handle.clone()),
        row.boxed.as_ref().and_then(|b| b.workdir.clone()),
    ) else {
        return Ok(None);
    };
    let workdir = recorded_workdir(&workdir)
        .ok_or_else(|| CommandError::new(ErrorKind::Internal, "recorded box workdir is invalid"))?;
    let cfg = settings(cx, Path::new(at), &Flags::default())?;
    let held = Decision::new(row.id).intent(IntentChange::Set(Intent::Setup {
        pid: std::process::id(),
    }));
    store.apply(held)?;
    let advance = row.state == TaskState::SettingUp;
    let target = SetupTarget {
        handle: &handle,
        workdir: &workdir,
        worktree: Path::new(at),
    };
    Ok(Some(run_and_record(
        cx, store, row.id, advance, &target, &cfg,
    )?))
}

/// Whether the Task failed setup on a Box that is still ready, so only the dotfiles run again.
fn dotfiles_retry(row: &TaskRow) -> bool {
    row.state == TaskState::SetupFailed
        && row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready)
}

/// Rebuilds the Box path from the workdir text stored on the Task row.
pub(crate) fn recorded_workdir(stored: &str) -> Option<BoxPath> {
    BoxPath::restore(stored).ok()
}

/// Whether the Task's Box is ready but its repo setup failed or its owner died.
fn setup_wanted(cx: &Cx, row: &TaskRow) -> bool {
    if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Ready) {
        return false;
    }
    if let Some(intent) = &row.intent {
        return setup_dead(intent, cx.proc().alive(intent.pid()));
    }
    matches!(row.state, TaskState::Working | TaskState::Idle) && row.setup_failed_at.is_some()
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
    let egress = egress_plan(cfg)?;
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
    let unenforced = warn_unenforced(cx, cfg);
    let worktree = PathBuf::from(task.worktree.as_deref().unwrap_or(&task.repo));
    let req = AcquireReq {
        provider,
        task: id,
        name: task.name,
        egress,
        class: Some(cfg.size.value.as_str().to_owned()),
        nonce: nonce(cx.rng()),
        worktree: task.worktree.as_deref().map(PathBuf::from),
    };
    match cx.driver().acquire(&req) {
        Ok(got) => {
            let applied = if unenforced {
                EgressApplied::NotEnforced
            } else {
                got.egress
            };
            record_acquired(store, id, &got, applied)?;
            let target = SetupTarget::new(&got, &worktree);
            run_and_record(cx, store, id, entry.begin().is_some(), &target, cfg)
        }
        Err(e) => Err(failed(cx, store, id, entry, &req, e)),
    }
}

/// Warns when the provider cannot apply the allow-list; returns whether it warned.
fn warn_unenforced(cx: &Cx, cfg: &TaskSettings) -> bool {
    let note = unenforced_note(&cfg.provider.value, cfg.enforce_egress);
    if let Some(note) = note {
        cx.warn(Warning {
            code: "egress_unenforced".to_owned(),
            message: note.to_owned(),
        });
    }
    note.is_some()
}

/// Six lowercase hex chars drawn from the RNG, so each attempt differs.
pub(crate) fn nonce(rng: &dyn Rng) -> String {
    format!("{:06x}", rng.u32() & 0xFF_FFFF)
}

/// Records an acquired Box under the setup intent.
fn record_acquired(
    store: &Store,
    id: TaskId,
    got: &Acquired,
    applied: EgressApplied,
) -> Result<(), CommandError> {
    let acquired = Decision::new(id)
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_handle: Field::Set(got.handle.clone()),
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            egress: Field::Set(applied.as_str().to_owned()),
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
    let run = setup::run_setup(cx, target.handle, target.workdir, target.worktree, cfg);
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

mod outcome;
mod retry;
#[cfg(test)]
mod tests;
