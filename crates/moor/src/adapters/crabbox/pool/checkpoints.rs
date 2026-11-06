//! Checkpoints of pool Boxes: their host keeps them, and moor only asks over the link.

use std::fs;

use moor_client::link::lease::{Answer, CheckpointAct, Verb};
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{AcquireReq, Checkpoint, CheckpointDisk, DriverError};

use super::{failed, planned, release, target, Crabbox, PoolHosts};
use crate::adapters::crabbox::{cache, size_of};

/// The failure of an answer that does not fit a checkpoint verb.
fn misfit(answer: &Answer) -> DriverError {
    failed(format!(
        "the host answered a checkpoint verb with {answer:?}"
    ))
}

/// The seam to pool hosts.
fn hosts(cb: &Crabbox) -> Result<&dyn PoolHosts, DriverError> {
    cb.pool_hosts
        .as_deref()
        .ok_or_else(|| failed("this controller reaches no pool hosts"))
}

/// Saves the Box behind `handle` as checkpoint `name` on its host.
pub(in crate::adapters::crabbox) fn create(
    cb: &Crabbox,
    handle: &BoxHandle,
    name: &str,
) -> Result<Checkpoint, DriverError> {
    let placed = cb.placed(handle)?;
    let act = CheckpointAct::CheckpointCreate {
        slug: placed.slug.to_owned(),
        name: name.to_owned(),
    };
    match placed
        .hosts
        .checkpoint(placed.machine, act)
        .map_err(failed)?
    {
        Answer::Checkpoint { id, bytes, parent } => Ok(Checkpoint { id, bytes, parent }),
        other => Err(misfit(&other)),
    }
}

/// Starts the lease `req` names from checkpoint `id` on its host and lets the controller's key
/// in, as a warmup does; a failure after the fork stops the lease.
pub(in crate::adapters::crabbox) fn fork(
    cb: &Crabbox,
    id: &str,
    req: &AcquireReq,
) -> Result<BoxHandle, DriverError> {
    let handle = planned(req)?;
    let placed = cb.placed(&handle)?;
    cb.forget_targets(&handle);
    let class = req.class.as_deref().map(size_of).transpose()?;
    let act = CheckpointAct::CheckpointFork {
        id: id.to_owned(),
        slug: placed.slug.to_owned(),
        cache: cache::lease(&req.cache),
        class: class.map(str::to_owned),
    };
    match placed
        .hosts
        .checkpoint(placed.machine, act)
        .map_err(failed)?
    {
        Answer::Done => {}
        other => return Err(misfit(&other)),
    }
    let reached = cb.controller_key().and_then(|key| {
        let public = fs::read_to_string(key.with_extension("pub"))
            .map_err(|e| failed(format!("cannot read the controller's public key: {e}")))?;
        placed.done(Verb::Authorize, Some(public.trim()))?;
        target(cb, &handle).map(drop)
    });
    match reached {
        Ok(()) => Ok(handle),
        Err(e) => {
            let _ = release(cb, &handle);
            Err(e)
        }
    }
}

/// Deletes checkpoint `id` from machine `machine`.
pub(in crate::adapters::crabbox) fn delete(
    cb: &Crabbox,
    machine: i64,
    id: &str,
) -> Result<(), DriverError> {
    let act = CheckpointAct::CheckpointDelete { id: id.to_owned() };
    match hosts(cb)?.checkpoint(machine, act).map_err(failed)? {
        Answer::Done => Ok(()),
        other => Err(misfit(&other)),
    }
}

/// The disk machine `machine` keeps checkpoints on.
pub(in crate::adapters::crabbox) fn disk(
    cb: &Crabbox,
    machine: i64,
) -> Result<CheckpointDisk, DriverError> {
    let act = CheckpointAct::Disk {};
    match hosts(cb)?.checkpoint(machine, act).map_err(failed)? {
        Answer::Disk {
            free_mib,
            total_mib,
        } => Ok(CheckpointDisk {
            free_mib,
            total_mib,
        }),
        other => Err(misfit(&other)),
    }
}

#[cfg(test)]
mod tests;
