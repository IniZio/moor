//! Checkpoints through `crabbox checkpoint`. Every read of its output lives here, since its
//! field names may still move.

use std::path::Path;
use std::time::Duration;

use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::driver::{AcquireReq, Checkpoint, CheckpointDisk, DriverError};
use serde::Deserialize;
use serde_json::Value;

use super::deadlines::QUICK_DEADLINE;
use super::errors::{unreadable, Op};
use super::{cache, caps, handle_of, pool, size_of, slug, split_handle, target_args, Crabbox};

impl Crabbox {
    /// Measures the disk of providers on this host at `dir`, the user's cache dir.
    pub(crate) fn with_checkpoint_dir(mut self, dir: Option<std::path::PathBuf>) -> Self {
        self.checkpoint_dir = dir;
        self
    }
}

/// Time allowed to save or fork a checkpoint, which copies a Box's memory and disk.
const CHECKPOINT_DEADLINE: Duration = Duration::from_mins(10);

/// What `checkpoint create --json` prints; unknown fields are ignored.
#[derive(Deserialize)]
struct Made {
    /// Checkpoint id, a string or a number.
    id: Value,
    /// The provider's own record of the checkpoint.
    #[serde(default)]
    native: Option<Native>,
}

/// The provider part of a checkpoint record.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Native {
    /// Size on the host.
    #[serde(default)]
    bytes: Option<u64>,
    /// What only this checkpoint holds, without the checkpoints it is a delta over.
    #[serde(default)]
    unique_bytes: Option<u64>,
    /// The checkpoint it is a delta over; absent over the base image.
    #[serde(default)]
    parent_checkpoint_id: Option<String>,
}

/// What `checkpoint fork --json` prints of the new lease; unknown fields are ignored.
#[derive(Deserialize)]
struct Forked {
    /// The lease's slug, when printed.
    #[serde(default)]
    slug: Option<String>,
}

/// Saves the Box behind `handle` as checkpoint `name`, from its lease dir.
pub(super) fn create(
    cb: &Crabbox,
    handle: &BoxHandle,
    name: &str,
) -> Result<Checkpoint, DriverError> {
    let spec = Crabbox::spec(CHECKPOINT_DEADLINE)
        .args(["checkpoint", "create"])
        .args(target_args(handle))
        .args(["--name", name, "--mode", "native", "--json"]);
    let spec = cb.at_lease(spec, handle)?;
    let out = cb.call(Op::Checkpoint, split_handle(handle).0, &spec)?;
    parse_made(&out.stdout).ok_or_else(|| unreadable("checkpoint"))
}

/// Starts the lease `req` names from checkpoint `id`, run from that lease's own dir so it is
/// claimed there, with the Box's cache volumes as warmup gives them; a fork that fails releases
/// what it started.
pub(super) fn fork(cb: &Crabbox, id: &str, req: &AcquireReq) -> Result<BoxHandle, DriverError> {
    let provider = req.provider.as_str();
    let name = slug(req);
    let class = size_of(req.class.as_deref())?;
    let handle = handle_of(provider, &name);
    let spec = cache::with_volumes(Crabbox::spec(CHECKPOINT_DEADLINE), &req.cache)
        .args(["checkpoint", "fork", id])
        .args(["--slug", &name, "--class", class, "--json"]);
    let existed = cb.lease_dir(&handle)?.exists();
    let spec = cb.at_lease(spec, &handle)?;
    cb.forget_targets(&handle);
    let forked = cb
        .call(Op::Acquire, Some(provider), &spec)
        .and_then(|out| parse_forked(&out.stdout, &name));
    match forked {
        Err(failed) if !existed => Err(cb.undo_acquire(&handle, failed)),
        Err(failed) => Err(failed),
        Ok(()) => Ok(handle),
    }
}

/// Deletes checkpoint `id` and the provider's copy of it.
pub(super) fn delete(cb: &Crabbox, id: &str) -> Result<(), DriverError> {
    cb.require_version()?;
    let spec = Crabbox::spec(CHECKPOINT_DEADLINE).args(["checkpoint", "delete", id]);
    cb.call(Op::Checkpoint, None, &spec).map(drop)
}

/// A provider's checkpoint disk, when it can be read.
pub(super) type Disk = Option<CheckpointDisk>;

/// The disk `provider` keeps checkpoints on: this host's, for a provider that runs here.
pub(super) fn disk(cb: &Crabbox, provider: &ProviderId) -> Result<Disk, DriverError> {
    let Some(dir) = cb.checkpoint_dir.as_deref() else {
        return Ok(None);
    };
    if pool::is_pool_provider(provider) {
        return Ok(None);
    }
    cb.require_version()?;
    let spec = Crabbox::spec(QUICK_DEADLINE).args(["providers", "--json", "--configured"]);
    let out = cb.call(Op::Caps, None, &spec)?;
    Ok(caps::on_this_host(&out.stdout, provider)
        .then(|| disk_at(dir))
        .flatten())
}

/// The filesystem holding `dir`, or its nearest existing parent; none when none can be read.
fn disk_at(dir: &Path) -> Option<CheckpointDisk> {
    let fs = dir.ancestors().find_map(|d| rustix::fs::statvfs(d).ok())?;
    let mib = |blocks: u64| blocks.saturating_mul(fs.f_frsize) / (1024 * 1024);
    Some(CheckpointDisk {
        free_mib: mib(fs.f_bavail),
        total_mib: mib(fs.f_blocks),
    })
}

/// The checkpoint `text` reports; none when it is unreadable or has no id.
fn parse_made(text: &str) -> Option<Checkpoint> {
    let made: Made = serde_json::from_str(text).ok()?;
    let id = match made.id {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let native = made.native;
    (!id.is_empty()).then_some(Checkpoint {
        id,
        bytes: native.as_ref().and_then(|n| n.unique_bytes.or(n.bytes)),
        parent: native.and_then(|n| n.parent_checkpoint_id),
    })
}

/// Checks the fork `text` reports is the lease moor named `slug`.
fn parse_forked(text: &str, slug: &str) -> Result<(), DriverError> {
    let forked: Forked = serde_json::from_str(text).map_err(|_| unreadable("checkpoint fork"))?;
    match forked.slug {
        Some(got) if got != slug => Err(DriverError::Failed {
            summary: format!("crabbox forked the checkpoint as {got:?}, not the asked {slug:?}"),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
