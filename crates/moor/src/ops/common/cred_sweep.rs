//! Removes credential directories a failed cleanup left on a Box.
//!
//! Run while the caller holds the Box lock, so no launch or exec of this Task is starting.

use std::path::Path;
use std::time::Duration;

use moor_core::box_key::CRED_SWEEP_SCRIPT;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::ExecSpec;
use moor_core::seam::proc::Deadline;

use crate::cx::Cx;
use crate::store::TaskRow;

/// Wall-clock allowance for the sweep.
const SWEEP_DEADLINE: Duration = Duration::from_secs(15);

/// Removes stale credential directories from the Task's Box; best effort, never fails.
pub(crate) fn sweep(cx: &Cx, row: &TaskRow) {
    let Some(boxed) = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready) else {
        return;
    };
    let Some(handle) = boxed.handle.as_ref() else {
        return;
    };
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let spec = ExecSpec {
        argv: vec!["sh".into(), "-c".into(), CRED_SWEEP_SCRIPT.into()],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(worktree.to_path_buf()),
        class: None,
    };
    let _ = cx.driver().exec(handle, &spec, Deadline(SWEEP_DEADLINE));
}

#[cfg(test)]
mod tests;
