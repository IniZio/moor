//! Removes credential directories a failed cleanup left on a Box.
//!
//! Run while the caller holds the Box lock, so no launch or exec of this Box is starting.

use std::path::Path;

use moor_core::box_key::CRED_SWEEP_SCRIPT;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::ExecSpec;
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

use crate::cx::Cx;
use crate::ops::common::box_exec::{retry_lost, StepError, STEP_DEADLINE};
use crate::store::BoxRow;

/// Removes stale credential directories from the Box, again while the ssh connection drops or
/// times out; best effort, never fails.
pub(crate) fn sweep(cx: &Cx, row: &BoxRow) {
    if let Some((handle, spec)) = plan(row) {
        let _ = retry_lost(cx, || attempt(cx, handle, &spec));
    }
}

/// [`sweep`] tried once, for a sleep: a settle tick may run one, and it never retries inline.
pub(crate) fn sweep_once(cx: &Cx, row: &BoxRow) {
    if let Some((handle, spec)) = plan(row) {
        let _ = attempt(cx, handle, &spec);
    }
}

/// The Box to sweep and the sweep command; none unless the Box is ready.
fn plan(row: &BoxRow) -> Option<(&BoxHandle, ExecSpec)> {
    let boxed = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    let handle = boxed.handle.as_ref()?;
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
    Some((handle, spec))
}

/// One run of the sweep.
fn attempt(cx: &Cx, handle: &BoxHandle, spec: &ExecSpec) -> Result<(), StepError> {
    match cx.driver().exec(handle, spec, Deadline(STEP_DEADLINE)) {
        Ok(_) => Ok(()),
        Err(e) => Err(StepError::from_driver(
            &e,
            format!("could not sweep stale credential directories: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests;
