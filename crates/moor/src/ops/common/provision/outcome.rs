//! What provisioning ran in a Box, and how a failed dotfiles step is recorded.

use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::task::TaskInput;
use moor_core::ErrorKind;
use moor_core::TaskId;

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

/// Records a failed dotfiles step and returns the error, carrying the Task row, to report.
///
/// The Box is kept. `advance` moves the Task to setup-failed; a Task already working gets a
/// `setup_failed` event instead.
pub(super) fn dotfiles_failed(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    advance: bool,
    failure: DotfilesFailed,
) -> CommandError {
    let tail = failure.tail;
    let mut done = Decision::new(id);
    if advance {
        done.task_input = Some(TaskInput::DotfilesFailed {
            error: tail.clone(),
        });
    } else {
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
    let out = CommandError::new(ErrorKind::Failed, format!("dotfiles failed:\n{tail}"));
    match reread(store, id) {
        Ok(row) => out.with_value(serde_json::json!({ "task": task_view(&row, cx.config()) })),
        Err(_) => out,
    }
}
