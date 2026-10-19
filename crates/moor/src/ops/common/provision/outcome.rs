//! What provisioning ran in a Box, and how a failed step is recorded and reported.

use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::task::TaskInput;
use moor_core::BoxId;
use moor_core::ErrorKind;
use moor_core::{BoxHandle, Warning};

use super::reread;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::dotfiles::{DotfilesFailed, DotfilesRun};
use crate::ops::common::setup::SetupRun;
use crate::ops::common::view::task_view;
use crate::store::Store;

/// What provisioning ran in the Box: repo setup, then dotfiles.
pub(crate) struct Provisioned {
    /// How repo setup went.
    pub(crate) setup: SetupRun,
    /// How the dotfiles step went.
    pub(crate) dotfiles: DotfilesRun,
}

impl Provisioned {
    /// Nothing ran.
    pub(crate) const fn skipped() -> Self {
        Self {
            setup: SetupRun::Skipped,
            dotfiles: DotfilesRun::Skipped,
        }
    }
}

/// Records a failed checkout sync and returns the error, carrying the Task row, to report.
///
/// The Box is kept and the agent does not start. `advance` moves the Task to setup-failed; a
/// Task already working gets a `setup_failed` event instead.
pub(super) fn sync_failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    handle: &BoxHandle,
    reason: String,
) -> CommandError {
    let mut done = Decision::new(id);
    if advance {
        done.task_input = Some(TaskInput::SyncFailed {
            error: reason.clone(),
        });
    } else {
        done = done.event(ExtraEvent::SetupFailed {
            line: reason.clone(),
        });
    }
    let done = done
        .set(Fields {
            setup_failed_at: Field::Set(cx.clock().now().as_millis()),
            ..Fields::default()
        })
        .intent(IntentChange::Clear);
    if let Err(write) = store.apply(done) {
        return write.into();
    }
    step_failed(cx, store, id, "sync", handle, reason)
}

/// The error for a provisioning step that failed in a Box that is kept for debugging.
///
/// `step` is `sync`, `setup` or `credential`. The message is the reason, which carries the
/// output tail when there is one; the hints say where the Box is, how to retry and how to close.
pub(super) fn step_failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    step: &str,
    handle: &BoxHandle,
    reason: String,
) -> CommandError {
    let log_tail = reason.clone();
    let out = CommandError::new(ErrorKind::Failed, reason);
    let Ok(row) = reread(store, id) else {
        return out;
    };
    let name = row.name.as_str().to_owned();
    out.with_hint(format!(
        "the {step} step failed; Box {} is kept for debugging and the agent did not start",
        handle.as_str()
    ))
    .with_hint(format!("`moor start {name}` again retries on this Box"))
    .with_hint(format!(
        "`moor close {name}` closes it; add --force to discard unpushed work in the Box"
    ))
    .with_value(serde_json::json!({
        "task": task_view(&row, cx.config()),
        "failed_step": step,
        "log_tail": log_tail,
        "box": handle.as_str(),
    }))
}

/// Warns that the dotfiles failed and returns the run to report; the Box carries on without them.
pub(super) fn dotfiles_warned(cx: &Cx, failure: DotfilesFailed) -> DotfilesRun {
    cx.warn(Warning {
        code: "dotfiles_failed".to_owned(),
        message: format!("dotfiles failed:\n{}", failure.tail),
    });
    DotfilesRun::Failed { tail: failure.tail }
}
