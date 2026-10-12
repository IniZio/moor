//! The Box step of `start`: acquire for a new Task, or again for a lost Box.

use std::path::PathBuf;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{
    AcquireReq, Acquired, DriverError, EgressApplied, EgressPlan, Released,
};
use moor_core::{BoxHandle, ProviderId, TaskId};

use super::{reread, task_view};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, TaskRow};

/// Where the Task stands when its Box is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    /// A new Task still being checked.
    Fresh,
    /// A Task whose setup failed.
    Retry,
    /// A working Task that lost its Box; its state stays.
    Stay,
}

impl Entry {
    /// The Task input that starts the step, if any.
    const fn begin(self) -> Option<TaskInput> {
        match self {
            Self::Fresh => Some(TaskInput::Admitted),
            Self::Retry => Some(TaskInput::Retry),
            Self::Stay => None,
        }
    }
}

/// Gives a Task with a failed setup or a lost Box a Box again.
///
/// Returns whether anything was provisioned.
pub(super) fn reprovision(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    let box_state = row.boxed.as_ref().map(|b| b.state);
    let entry = match (row.state, box_state) {
        (TaskState::SetupFailed, Some(BoxState::Ready)) => {
            store.apply(Decision::new(row.id).task_input(TaskInput::Retry))?;
            let ok = TaskInput::BoxReady {
                setup: SetupOutcome::Ok,
            };
            store.apply(Decision::new(row.id).task_input(ok))?;
            return Ok(true);
        }
        (TaskState::SetupFailed, Some(BoxState::Failed | BoxState::Gone)) => Entry::Retry,
        (TaskState::Working | TaskState::Idle, Some(BoxState::Failed | BoxState::Gone)) => {
            Entry::Stay
        }
        _ => return Ok(false),
    };
    provision(cx, store, row.id, entry)?;
    Ok(true)
}

/// Admits the Task, acquires its Box and records the outcome.
pub(super) fn provision(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    entry: Entry,
) -> Result<(), CommandError> {
    let task = reread(store, id)?;
    let provider = ProviderId::new(cx.config().provider_or_default());
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
    store.apply(requested)?;
    let req = AcquireReq {
        provider,
        task: id,
        name: task.name,
        egress: EgressPlan::Open,
        class: cx.config().box_.size.clone(),
        nonce: nonce(cx.clock().now()),
        worktree: task.worktree.as_deref().map(PathBuf::from),
    };
    match cx.driver().acquire(&req) {
        Ok(got) => ready(store, id, entry, got),
        Err(e) => Err(failed(cx, store, id, entry, &req, e)),
    }
}

/// Six lowercase hex chars from the clock, so each attempt differs.
fn nonce(now: Ts) -> String {
    format!("{:06x}", now.as_millis().rem_euclid(0x100_0000))
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
