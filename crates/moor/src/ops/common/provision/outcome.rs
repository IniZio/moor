//! What provisioning ran in a Box, and how a failed dotfiles step is recorded.

use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::task::TaskInput;
use moor_core::BoxId;
use moor_core::ErrorKind;

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
    let out = CommandError::new(ErrorKind::Failed, reason);
    match reread(store, id) {
        Ok(row) => out.with_value(serde_json::json!({ "task": task_view(&row, cx.config()) })),
        Err(_) => out,
    }
}

/// Records a failed dotfiles step and returns the error, carrying the Task row, to report.
///
/// The Box is kept. `advance` moves the Task to setup-failed; a Task already working gets a
/// `setup_failed` event instead. A repo setup line that failed earlier in the same step is
/// kept as its own `setup_failed` event and named in the error, so neither failure is lost.
pub(super) fn dotfiles_failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    setup_line: Option<String>,
    failure: DotfilesFailed,
) -> CommandError {
    let tail = failure.tail;
    let mut done = Decision::new(id);
    if advance {
        done.task_input = Some(TaskInput::DotfilesFailed {
            error: tail.clone(),
        });
    }
    if let Some(line) = &setup_line {
        done = done.event(ExtraEvent::SetupFailed { line: line.clone() });
    }
    if !advance {
        done = done.event(ExtraEvent::SetupFailed { line: tail.clone() });
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
    let setup = setup_line.map_or_else(String::new, |l| format!("setup failed: {l}\n"));
    let out = CommandError::new(
        ErrorKind::Failed,
        format!("{setup}dotfiles failed:\n{tail}"),
    );
    match reread(store, id) {
        Ok(row) => out.with_value(serde_json::json!({ "task": task_view(&row, cx.config()) })),
        Err(_) => out,
    }
}
