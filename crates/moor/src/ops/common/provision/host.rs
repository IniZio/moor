//! The repo's host steps during provisioning, which run before setup so its lines can reach them.

use moor_core::config::TaskSettings;
use moor_core::task_key::TaskKey;
use moor_core::TaskId;

use super::{reread, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::host_steps::{backend_up, HostSite};
use crate::store::Store;

/// Starts the repo's backend on the host when `box.host_steps` asks for it.
///
/// Off macOS the platform check has already refused such a repo, so this is only reached on a Mac.
///
/// # Errors
/// The backend did not start, or the Task row is gone; the message is the step's own.
pub(super) fn run_host_steps(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    target: &SetupTarget<'_>,
    cfg: &TaskSettings,
) -> Result<(), CommandError> {
    if !cfg.repo_box.wants_backend() {
        return Ok(());
    }
    let key = TaskKey::new(&reread(store, id)?.name);
    let site = HostSite {
        handle: target.handle,
        workdir: target.workdir,
        key: &key,
        worktree: target.worktree,
    };
    backend_up(cx, id, &site, cfg)
}
