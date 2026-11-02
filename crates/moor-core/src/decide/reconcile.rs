//! The reconcile decider: Boxes the store holds but the Driver no longer lists.

use std::collections::HashSet;

use crate::domain::box_state::BoxLifecycleInput;
use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::Decision;
use crate::name::{BoxHandle, BoxId};
use crate::seam::driver::BoxListing;

/// One stored Box row, as the op read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxFact {
    /// The owning box.
    pub box_id: BoxId,
    /// The Box state.
    pub state: BoxState,
    /// The stored Driver handle, if any.
    pub handle: Option<BoxHandle>,
}

/// Why a reconcile sleeps a Box its provider lists as stopped.
pub const STOPPED_REASON: &str = "its machine stopped, as when its host restarts";

/// Decides which rows to mark Gone or Asleep given a successful Driver listing.
///
/// Only `Ready` and `Asleep` rows with a handle can go missing: a `Starting`
/// row is mid-acquire (its handle may not exist yet), and other states are
/// already past needing the Box. No lifecycle input is emitted for a missing Box: the lifecycle
/// machine has no Box-gone transition. A row is live only when its stored handle is listed.
/// A Ready row listed asleep is slept, as [`stopped`] decides.
/// A failed listing must not call this; the op then decides nothing.
#[must_use]
pub fn reconcile(rows: &[BoxFact], listing: &[BoxListing]) -> Vec<Decision> {
    let listed: HashSet<&BoxHandle> = listing.iter().map(|l| &l.handle).collect();
    rows.iter()
        .filter(|r| matches!(r.state, BoxState::Ready | BoxState::Asleep))
        .filter(|r| r.handle.as_ref().is_some_and(|h| !listed.contains(h)))
        .map(|r| Decision::new(r.box_id).box_input(BoxInput::DriverMissing))
        .chain(stopped(rows, listing))
        .collect()
}

/// Sleeps each Ready row whose handle the listing shows asleep, so a wake boots it again with
/// its disk; a row the listing leaves out is not decided here.
#[must_use]
pub fn stopped(rows: &[BoxFact], listing: &[BoxListing]) -> Vec<Decision> {
    let asleep: HashSet<&BoxHandle> = listing
        .iter()
        .filter(|l| l.asleep)
        .map(|l| &l.handle)
        .collect();
    rows.iter()
        .filter(|r| r.state == BoxState::Ready)
        .filter(|r| r.handle.as_ref().is_some_and(|h| asleep.contains(h)))
        .map(|r| {
            Decision::new(r.box_id)
                .lifecycle(BoxLifecycleInput::Sleep {
                    auto: true,
                    reason: Some(STOPPED_REASON.to_owned()),
                })
                .box_input(BoxInput::Sleep)
        })
        .collect()
}

#[cfg(test)]
mod tests;
