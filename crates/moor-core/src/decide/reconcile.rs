//! The reconcile decider: Boxes the store holds but the Driver no longer lists.

use std::collections::HashSet;

use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::Decision;
use crate::name::{BoxHandle, TaskId};
use crate::seam::driver::BoxListing;

/// One stored Box row, as the op read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxFact {
    /// The owning task.
    pub task: TaskId,
    /// The Box state.
    pub state: BoxState,
    /// The stored Driver handle, if any.
    pub handle: Option<BoxHandle>,
}

/// Decides which rows to mark Gone given a successful Driver listing.
///
/// Only `Ready` and `Asleep` rows with a handle can go missing: a `Starting`
/// row is mid-acquire (its handle may not exist yet), and other states are
/// already past needing the Box. No task input is emitted: the task machine
/// has no Box-gone transition. An asleep/awake mismatch with the provider is not handled yet.
/// A failed listing must not call this; the op then decides nothing.
#[must_use]
pub fn reconcile(rows: &[BoxFact], listing: &[BoxListing]) -> Vec<Decision> {
    let listed: HashSet<&BoxHandle> = listing.iter().map(|l| &l.handle).collect();
    rows.iter()
        .filter(|r| matches!(r.state, BoxState::Ready | BoxState::Asleep))
        .filter(|r| r.handle.as_ref().is_some_and(|h| !listed.contains(h)))
        .map(|r| Decision::new(r.task).box_input(BoxInput::DriverMissing))
        .collect()
}

#[cfg(test)]
mod tests;
