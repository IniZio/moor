//! Room on the disk a provider keeps setup checkpoints on. Below the node's disk floor, the
//! checkpoints no Box holds back are deleted, superseded chains first, then the least recently
//! used, until the disk is above the floor again.

use std::collections::HashSet;

use moor_core::decide::schedule::disk_floor_mib;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::driver::DriverError;
use moor_core::{BoxId, ProviderId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// Deletes checkpoints under `provider` while its disk is below the floor, each told in an event
/// of Box `id`; returns whether the disk is above the floor, or does not report itself.
pub(super) fn make_room(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    provider: &ProviderId,
) -> Result<bool, CommandError> {
    let mut failed = HashSet::new();
    loop {
        let Some(disk) = cx.driver().checkpoint_disk(provider).ok().flatten() else {
            return Ok(true);
        };
        let needs_mib = disk_floor_mib(disk.total_mib);
        if disk.free_mib >= needs_mib {
            return Ok(true);
        }
        let mut evictable = store.evictable(provider)?.into_iter();
        let Some(next) = evictable.find(|e| !failed.contains(&e.checkpoint)) else {
            return Ok(false);
        };
        let Some(claimed) = store.claim_checkpoint(provider, &next)? else {
            continue;
        };
        match cx.driver().delete_checkpoint(&next.checkpoint) {
            Ok(()) | Err(DriverError::NotFound) => {}
            Err(e) => {
                store.restore_checkpoint(&claimed)?;
                failed.insert(next.checkpoint.clone());
                cx.warn(Warning {
                    code: "checkpoint_delete_failed".to_owned(),
                    message: format!(
                        "could not delete setup checkpoint {} to free disk: {e}; trying again later",
                        next.checkpoint
                    ),
                });
                continue;
            }
        }
        store.apply(Decision::new(id).event(ExtraEvent::CheckpointEvicted {
            checkpoint: next.checkpoint,
            repo: next.repo,
            bytes: next.bytes,
            reason: next.reason.as_str().to_owned(),
            free_mib: disk.free_mib,
            needs_mib,
        }))?;
    }
}
