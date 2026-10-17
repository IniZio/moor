//! What provisioning ran in a Box, and how a failed dotfiles step is recorded.

use std::path::Path;

use moor_core::config::{select_toolchains, RepoBox, TaskSettings};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::task::TaskInput;
use moor_core::seam::driver::{Acquired, Platform};
use moor_core::ErrorKind;
use moor_core::TaskId;

use super::reread;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::dotfiles::{DotfilesFailed, DotfilesRun};
use crate::ops::common::setup::SetupRun;
use crate::ops::common::simulator;
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

/// Why the repo's `box:` section cannot apply to a Box of `platform`, worded as a config error.
pub(super) fn platform_refusal(cx: &Cx, repo_box: &RepoBox, platform: Platform) -> Option<String> {
    let macos = platform == Platform::Macos;
    let e = repo_box.check_platform(macos).err().or_else(|| {
        macos
            .then(|| select_toolchains(&cx.config().toolchains, &repo_box.toolchains).err())
            .flatten()
    })?;
    Some(e.in_file(".moor.yaml").to_string())
}

/// Why provisioning stops before setup: the Box cannot take the repo's `box:` section, or the
/// Task's simulator cannot be made. `None` when setup may run.
pub(super) fn refusal(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    got: &Acquired,
    worktree: &Path,
    cfg: &TaskSettings,
) -> Option<String> {
    platform_refusal(cx, &cfg.repo_box, got.platform)
        .or_else(|| simulator::ensure(cx, store, id, got, worktree, cfg).err())
}

/// Records a failed checkout sync and returns the error, carrying the Task row, to report.
///
/// The Box is kept and the agent does not start. `advance` moves the Task to setup-failed; a
/// Task already working gets a `setup_failed` event instead.
pub(super) fn sync_failed(
    cx: &Cx,
    store: &Store,
    id: TaskId,
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
    id: TaskId,
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
