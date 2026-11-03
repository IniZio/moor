//! Checkpoints through `crabbox checkpoint`. Every read of its output lives here, since its
//! field names may still move.

use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{AcquireReq, Checkpoint, DriverError};
use serde::Deserialize;
use serde_json::Value;

use super::errors::{unreadable, Op};
use super::{handle_of, slug, split_handle, target_args, Crabbox};

/// Time allowed to save or fork a checkpoint, which copies a Box's memory and disk.
const CHECKPOINT_DEADLINE: Duration = Duration::from_mins(10);

/// What `checkpoint create --json` prints; unknown fields are ignored.
#[derive(Deserialize)]
struct Made {
    /// Checkpoint id, a string or a number.
    id: Value,
    /// Size on the host.
    #[serde(default)]
    bytes: Option<u64>,
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
/// claimed there; a fork that fails releases what it started.
pub(super) fn fork(cb: &Crabbox, id: &str, req: &AcquireReq) -> Result<BoxHandle, DriverError> {
    let provider = req.provider.as_str();
    let name = slug(req);
    let handle = handle_of(provider, &name);
    let spec = Crabbox::spec(CHECKPOINT_DEADLINE)
        .args(["checkpoint", "fork", id])
        .args(["--slug", &name, "--json"]);
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

/// The checkpoint `text` reports; none when it is unreadable or has no id.
fn parse_made(text: &str) -> Option<Checkpoint> {
    let made: Made = serde_json::from_str(text).ok()?;
    let id = match made.id {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!id.is_empty()).then_some(Checkpoint {
        id,
        bytes: made.bytes,
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
