//! Box provisioning shared by the task ops: acquire for a new Task, or again for a lost Box.

use std::path::{Path, PathBuf};

use moor_core::config::egress::{effective_allow_list, provider_applies_policy, unenforced_note};
use moor_core::config::{Flags, TaskSettings};

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{
    AcquireReq, Acquired, DriverError, EgressApplied, EgressPlan, Released,
};
use moor_core::seam::rng::Rng;
use moor_core::{BoxHandle, ErrorKind, ProviderId, TaskId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::capacity;
use crate::ops::common::facts::settings;
use crate::ops::common::view::task_view;
use crate::store::{Store, TaskRow};

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
/// Returns whether anything was provisioned. The Box lock is held while
/// provisioning and the row is read again under it; another op holding it is
/// refused as busy.
pub(crate) fn reprovision(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    op: &str,
) -> Result<bool, CommandError> {
    let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
    if row.state == TaskState::SetupFailed && ready {
        store.apply(Decision::new(row.id).task_input(TaskInput::Retry))?;
        let ok = TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        };
        store.apply(Decision::new(row.id).task_input(ok))?;
        return Ok(true);
    }
    if entry_for(row).is_none() {
        return Ok(false);
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, op, boxlock::WAIT)?;
    let row = reread(store, row.id)?;
    let Some(entry) = entry_for(&row) else {
        return Ok(false);
    };
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    let cfg = settings(cx, Path::new(at), &Flags::default())?;
    provision(cx, store, row.id, entry, &cfg)?;
    Ok(true)
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
) -> Result<(), CommandError> {
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
            return Ok(());
        }
    } else {
        store.apply(requested)?;
    }
    if let Some(note) = unenforced_note(&cfg.provider.value, cfg.enforce_egress) {
        cx.warn(Warning {
            code: "egress_unenforced".to_owned(),
            message: note.to_owned(),
        });
    }
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
        Ok(got) => ready(store, id, entry, got),
        Err(e) => Err(failed(cx, store, id, entry, &req, e)),
    }
}

/// Six lowercase hex chars drawn from the RNG, so each attempt differs.
pub(crate) fn nonce(rng: &dyn Rng) -> String {
    format!("{:06x}", rng.u32() & 0xFF_FFFF)
}

/// Records an acquired Box and moves the Task to working.
fn ready(store: &Store, id: TaskId, entry: Entry, got: Acquired) -> Result<(), CommandError> {
    let mut done = Decision::new(id);
    done.task_input = entry.begin().map(|_| TaskInput::BoxReady {
        setup: SetupOutcome::Ok,
    });
    let done = done
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_handle: Field::Set(got.handle),
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            egress_enforced: Field::Set(got.egress == EgressApplied::Enforced),
            ..Fields::default()
        })
        .intent(IntentChange::Clear);
    store.apply(done)?;
    Ok(())
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
        Ok(row) => out.with_value(serde_json::json!({ "task": task_view(&row) })),
        Err(_) => out,
    }
}
