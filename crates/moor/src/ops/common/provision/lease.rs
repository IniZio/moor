//! Lease rows that track the machine a Box acquires.

use moor_core::seam::driver::Acquired;
use moor_core::{BoxHandle, BoxId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::lease::{LeaseId, LeaseKind, LeaseState};
use crate::store::Store;

/// Holds the planned root when the Box came back under the planned handle. Otherwise
/// nothing exists under the planned key, so that row stays `planned` and the acquired
/// handle gets its own held row.
pub(crate) fn hold_root(
    store: &Store,
    id: BoxId,
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
pub(crate) fn hold_workdir(store: &Store, id: BoxId, got: &Acquired) -> Result<(), CommandError> {
    let dir = store.plan_lease(id, LeaseKind::BoxDir, got.workdir.as_str())?;
    store.hold_lease(dir)?;
    Ok(())
}

/// The handle the Box's latest acquire planned; the machine may exist under it before any
/// handle is recorded, since an acquire can stop part-way.
pub(crate) fn planned_root(store: &Store, id: BoxId) -> Result<Option<BoxHandle>, CommandError> {
    let latest = store
        .leases(id)?
        .into_iter()
        .filter(|l| l.kind == LeaseKind::BoxRoot && l.state == LeaseState::Planned)
        .max_by_key(|l| l.id.0);
    Ok(latest.map(|l| BoxHandle::new(l.key)))
}

/// Releases the machine of each hands Box `id` left and records the release on their row. A
/// release cut off by a crash or a failure stays owed, and the Box's next resume does it. One that
/// will not release keeps its lease record and is warned about with how to release it by hand.
pub(super) fn release_left(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    for (inc, old) in store.hands_left(id)? {
        if let Err(e) = cx.driver().release(&old) {
            let by_hand = cx.driver().manual_release(&old).unwrap_or_default();
            cx.warn(Warning {
                code: "release_failed".to_owned(),
                message: format!("could not release the machine the Box left: {e}. {by_hand}"),
            });
            continue;
        }
        for lease in store.leases(id)? {
            if lease.kind == LeaseKind::BoxRoot && lease.key == old.as_str() {
                store.drop_lease(lease.id)?;
            }
        }
        store.left_hands_released(id, inc)?;
    }
    Ok(())
}
