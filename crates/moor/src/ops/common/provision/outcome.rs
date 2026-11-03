//! What provisioning ran in a Box, and how a failed step is recorded and reported.

use moor_core::domain::decision::{Decision, Field, Fields, IntentChange};
use moor_core::view::WorkTreeView;
use moor_core::BoxId;
use moor_core::ErrorKind;
use moor_core::{BoxHandle, Warning};

use super::{reread, step_failed};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::dotfiles::{DotfilesFailed, DotfilesRun};
use crate::ops::common::setup::SetupRun;
use crate::ops::common::view::box_view;
use crate::store::Store;

/// What provisioning ran in the Box: repo setup, then dotfiles.
pub(crate) struct Provisioned {
    /// How repo setup went.
    pub(crate) setup: SetupRun,
    /// How the dotfiles step went.
    pub(crate) dotfiles: DotfilesRun,
    /// How the kept work tree started; none when the Box keeps none or already refreshed it.
    pub(crate) work_tree: Option<WorkTreeView>,
    /// Whether the agent install failed after every setup line passed.
    pub(crate) install_failed: bool,
}

impl Provisioned {
    /// Nothing ran.
    pub(crate) const fn skipped() -> Self {
        Self {
            setup: SetupRun::Skipped,
            dotfiles: DotfilesRun::Skipped,
            work_tree: None,
            install_failed: false,
        }
    }

    /// The step that held the Box in setup-failed, if one did.
    pub(crate) const fn failed_step(&self) -> Option<&'static str> {
        if self.setup.failed() {
            Some("setup")
        } else if self.install_failed {
            Some("agent_install")
        } else {
            None
        }
    }
}

/// Records a failed clone and returns the error, carrying the Box row, to report.
///
/// The Box is kept, moves to setup-failed, working or not, and the agent does not start.
pub(super) fn clone_failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    handle: &BoxHandle,
    reason: String,
) -> CommandError {
    let done = step_failed(Decision::new(id), advance, reason.clone())
        .set(Fields {
            setup_failed_at: Field::Set(cx.clock().now().as_millis()),
            ..Fields::default()
        })
        .intent(IntentChange::Clear);
    if let Err(write) = store.apply(done) {
        return write.into();
    }
    clone_step_failed(cx, store, id, handle, reason)
}

/// The error for a failed clone step in a Box that is kept for debugging.
///
/// The message is the reason, which carries the
/// output tail when there is one; the hints say where the Box is, how to retry and how to close.
fn clone_step_failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    handle: &BoxHandle,
    reason: String,
) -> CommandError {
    let out = CommandError::new(ErrorKind::Failed, reason);
    let Ok(row) = reread(store, id) else {
        return out;
    };
    let name = row.name.as_str().to_owned();
    out.with_hint(format!(
        "the clone step failed; Box {} is kept for debugging and the agent did not start",
        handle.as_str()
    ))
    .with_hint(format!("`moor start {name}` again retries on this Box"))
    .with_hint(format!(
        "`moor close {name}` closes it; add --force to discard unpushed work in the Box"
    ))
    .with_value(serde_json::json!({
        "box": box_view(&row, cx.config()),
        "failed_step": "clone",
        "handle": handle.as_str(),
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
