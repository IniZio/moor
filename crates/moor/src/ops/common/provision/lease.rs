//! Lease rows that track the Box a task acquires.

use moor_core::seam::driver::Acquired;
use moor_core::{BoxHandle, TaskId};

use crate::error::CommandError;
use crate::store::lease::{LeaseId, LeaseKind};
use crate::store::Store;

/// Holds the planned root when the Box came back under the planned handle. Otherwise
/// nothing exists under the planned key, so that row stays `planned` and the acquired
/// handle gets its own held row.
pub(crate) fn hold_root(
    store: &Store,
    id: TaskId,
    root: LeaseId,
    planned: &BoxHandle,
    got: &Acquired,
) -> Result<(), CommandError> {
    if got.handle == *planned {
        store.hold_lease(root)?;
    } else {
        let actual = store.plan_lease(id, LeaseKind::BoxRoot, got.handle.as_str())?;
        store.hold_lease(actual)?;
    }
    Ok(())
}

/// Plans and holds the lease on the acquired Box's working directory.
pub(crate) fn hold_workdir(store: &Store, id: TaskId, got: &Acquired) -> Result<(), CommandError> {
    let dir = store.plan_lease(id, LeaseKind::BoxDir, got.workdir.as_str())?;
    store.hold_lease(dir)?;
    Ok(())
}
