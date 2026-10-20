//! Removes credential directories a failed cleanup left on a Box.
//!
//! Run while the caller holds the Box lock, so no launch or exec of this Box is starting.

use std::path::Path;

use moor_core::box_key::CRED_SWEEP_SCRIPT;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::{BoxStep, DriverError};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, Warning};

use crate::cx::Cx;
use crate::ops::common::box_exec::{retry_lost, StepError, STEP_DEADLINE};
use crate::store::BoxRow;

/// Removes stale credential directories from the Box, again while the ssh connection drops or
/// times out; never fails, and warns when the last try fails.
pub(crate) fn sweep(cx: &Cx, row: &BoxRow) {
    if let Some((handle, worktree)) = target(row) {
        if let Err(message) = retry_lost(cx, || attempt(cx, handle, worktree)) {
            warn(cx, message);
        }
    }
}

/// [`sweep`] tried once, for a sleep: a settle tick may run one, and it never retries inline.
pub(crate) fn sweep_once(cx: &Cx, row: &BoxRow) {
    if let Some((handle, worktree)) = target(row) {
        if let Err(StepError::Lost(message) | StepError::Other(message)) =
            attempt(cx, handle, worktree)
        {
            warn(cx, message);
        }
    }
}

/// The Box to sweep and the host directory its driver runs from; none unless the Box is ready.
fn target(row: &BoxRow) -> Option<(&BoxHandle, &Path)> {
    let boxed = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    let handle = boxed.handle.as_ref()?;
    Some((
        handle,
        Path::new(row.worktree.as_deref().unwrap_or(&row.repo)),
    ))
}

/// One run of the sweep script over the Box's shared ssh connection, not through `crabbox run`.
fn attempt(cx: &Cx, handle: &BoxHandle, worktree: &Path) -> Result<(), StepError> {
    let text = "could not sweep stale credential directories";
    let failed = |e: &DriverError| StepError::from_driver(e, format!("{text}: {e}"));
    let steps = [BoxStep::Run(CRED_SWEEP_SCRIPT.to_owned())];
    let deadline = Deadline(STEP_DEADLINE);
    match cx
        .driver()
        .batch(handle, &[], &steps, worktree, deadline)
        .pop()
    {
        Some(Err(e)) => Err(failed(&e)),
        _ => Ok(()),
    }
}

/// Warns that stale credential directories may be left on the Box.
fn warn(cx: &Cx, message: String) {
    cx.warn(Warning {
        code: "cred_sweep_failed".to_owned(),
        message,
    });
}

#[cfg(test)]
mod tests;
