//! Room on the disk a provider keeps setup checkpoints on. Below the node's disk floor, the
//! checkpoints no Box holds back and no other checkpoint is a delta over are deleted, superseded
//! chains first, then the least recently used, until the disk is above the floor again.

use std::collections::HashSet;

use moor_core::box_key::PoolSeat;
use moor_core::decide::schedule::{disk_floor_mib, Node, Scope};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::request::{MachineLink, WarmStarts};
use moor_core::seam::driver::DriverError;
use moor_core::{BoxId, ProviderId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::node::runtime_id;
use crate::store::Store;

/// Deletes checkpoints under `provider` while its disk is below the floor, each told in an event
/// of Box `id`; returns whether the disk is above the floor, or does not report itself. First it
/// deletes any checkpoint claimed by a call cut short before its delete was done.
pub(super) fn make_room(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    provider: &ProviderId,
) -> Result<bool, CommandError> {
    let mut left = store.evicting(provider)?;
    let mut failed = HashSet::new();
    loop {
        let Some(disk) = cx.driver().checkpoint_disk(provider).ok().flatten() else {
            return Ok(true);
        };
        let needs_mib = disk_floor_mib(disk.total_mib);
        let next = match left.pop() {
            Some(next) => next,
            None if disk.free_mib >= needs_mib => return Ok(true),
            None => {
                let mut evictable = store.evictable(provider)?.into_iter();
                let Some(next) = evictable.find(|e| !failed.contains(&e.checkpoint)) else {
                    return Ok(false);
                };
                if !store.claim_checkpoint(provider, &next)? {
                    continue;
                }
                next
            }
        };
        match cx.driver().delete_checkpoint(provider, &next.checkpoint) {
            Ok(()) | Err(DriverError::NotFound) => {}
            Err(e) => {
                store.unclaim_checkpoint(provider, &next.checkpoint)?;
                cx.warn(Warning {
                    code: "checkpoint_delete_failed".to_owned(),
                    message: format!(
                        "could not delete setup checkpoint {} to free disk: {e}; trying again later",
                        next.checkpoint
                    ),
                });
                failed.insert(next.checkpoint);
                continue;
            }
        }
        if !store.forget_checkpoint(provider, &next.checkpoint)? {
            continue;
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

/// Frees disk on a personal node below its floor, on its agent's report. Such a node admits no Box,
/// so no start would get as far as making room. Nothing to tell a Box of and nothing to free when
/// no Box waits or runs there.
pub(crate) fn relieve_disk(cx: &Cx, store: &Store, node: &Node, link: &MachineLink) {
    let Scope::Personal { machine_id, .. } = node.scope else {
        return;
    };
    let below = node.memory.is_some_and(|m| {
        let (free, total) = (m.disk_free_mib, m.disk_total_mib);
        free.zip(total).is_some_and(|(f, t)| f < disk_floor_mib(t))
    });
    if !below || link.warm_starts != Some(WarmStarts::On) {
        return;
    }
    let told = (|| {
        let waiting = store.pending_on(&node.name)?.into_iter().next();
        let running = store.one_live_per_owner_on(&node.name)?.into_iter().next();
        let Some(id) = waiting.or(running) else {
            return Ok(true);
        };
        let seat = PoolSeat {
            machine: machine_id,
            controller: store.controller_id()?,
        };
        make_room(cx, store, id, &seat.checkpoint_home(&runtime_id(node)))
    })();
    if let Err(e) = told {
        let e: CommandError = e;
        cx.warn(Warning {
            code: "checkpoint_delete_failed".to_owned(),
            message: format!("cannot free disk on {}: {}", node.name, e.error),
        });
    }
}
