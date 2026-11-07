//! A retry moved onto a fork of its nearest kept layer, taken over when it was cut off.

use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::seam::driver::{AcquireReq, DriverError, Forked};
use moor_core::{BoxHandle, BoxId, ErrorKind, Warning};

use super::fork::{failed_at, nearest_of_own_base, release_old};
use super::Layered;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::disk::make_room;
use crate::ops::common::provision::progress::{goes_on_after, take_over};
use crate::ops::common::setup::FORCED_CLOSE;
use crate::store::checkpoints::CheckpointRow;
use crate::store::lease::{LeaseId, LeaseKind};
use crate::store::{BoxRow, Store};

/// Moves Box `row` from the machine `old` onto a new one forked from the nearest checkpoint of its
/// own base at or below its failed layer, and releases `old`. Returns the new handle; none when
/// no such checkpoint is kept or the fork fails, and the setup runs in `old` from its start.
///
/// With `adopt` the fork an earlier process left going is taken over instead. So is a fork cut off
/// by a lost link. A machine taken over may not be the fork, so its setup runs from the start.
pub(in super::super) fn resume(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    old: &BoxHandle,
    cfg: &BoxSettings,
    (layered, adopt): (&mut Layered, bool),
) -> Result<Option<BoxHandle>, CommandError> {
    let Some((layer, saved)) = kept_layer(cx, store, row, cfg, layered)? else {
        return Ok(None);
    };
    let req = layered.machine_req(cx, store, row, cfg)?;
    // Planned first, so a fork cut short is still found from its record.
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(row.id, LeaseKind::BoxRoot, planned.as_str())?;
    let spent = saved.place.as_ref().map_or(0, |p| p.setup_ms);
    let began = cx.clock().now().as_millis();
    let (new, told) = match fork_unless_forced(cx, &saved.checkpoint, &req, adopt)? {
        Err(e) if goes_on_after(cx, &req, &e) => {
            return took_over(cx, store, (row, old), &req, (root, &planned));
        }
        Ok(forked) => {
            let fork_ms = u64::try_from(cx.clock().now().as_millis() - began).unwrap_or_default();
            let told = ExtraEvent::WarmStarted {
                layer: u32::try_from(layer).unwrap_or(u32::MAX),
                layers: u32::try_from(layered.keys.len() - 1).unwrap_or(u32::MAX),
                saved_ms: spent.saturating_sub(fork_ms),
                fork_ms,
                steps: forked.steps,
                rerun_from: None,
                finished: false,
            };
            (forked.handle, told)
        }
        Err(e) => {
            cx.warn(Warning {
                code: "checkpoint_failed".to_owned(),
                message: format!(
                    "could not start from setup layer {layer}: {e}; setup runs from its start"
                ),
            });
            return Ok(None);
        }
    };
    hold_new(store, row.id, (root, &planned), &new)?;
    // One write, so a restart never reads this machine with the fork of the one before it.
    let moved = Decision::new(row.id).event(told).set(Fields {
        box_handle: Field::Set(new.clone()),
        ..Fields::default()
    });
    store.apply(moved)?;
    layered.stand_on(layer, spent);
    release_old(cx, store, row.id, old)?;
    Ok(Some(new))
}

/// The nearest layer of `row`'s own base at or below its failed one with a checkpoint, pinned and in
/// use; none when a retry has nothing to fork.
fn kept_layer(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    layered: &Layered,
) -> Result<Option<(usize, CheckpointRow)>, CommandError> {
    make_room(cx, store, row.id, &layered.home)?;
    if layered.replacing {
        return Ok(None);
    }
    let failed = failed_at(store, row, cfg)?;
    let Some((layer, saved)) = nearest_of_own_base(store, layered, failed)? else {
        return Ok(None);
    };
    let Some(key) = layered.keys.get(layer) else {
        return Ok(None);
    };
    store.pin_checkpoint(row.id, &layered.home, key, layer)?;
    if !store.use_checkpoint(&layered.home, key)? {
        return Ok(None);
    }
    Ok(Some((layer, saved)))
}

/// Forks `checkpoint` for `req`, or with `adopt` leaves the fork going to be taken over. A forced
/// close asked for meanwhile ends the retry; it releases what the fork made from its planned lease.
fn fork_unless_forced(
    cx: &Cx,
    checkpoint: &str,
    req: &AcquireReq,
    adopt: bool,
) -> Result<Result<Forked, DriverError>, CommandError> {
    let forked = if adopt {
        Err(DriverError::Transport {
            stderr_tail: "taken over".to_owned(),
        })
    } else {
        cx.driver().fork(checkpoint, req)
    };
    if cx.env().cancel().is_cancelled() {
        return Err(CommandError::new(ErrorKind::Failed, FORCED_CLOSE));
    }
    Ok(forked)
}

/// Holds the lease of `new`, the machine a fork planned as `planned` came to.
fn hold_new(
    store: &Store,
    id: BoxId,
    (root, planned): (LeaseId, &BoxHandle),
    new: &BoxHandle,
) -> Result<(), CommandError> {
    if new == planned {
        store.hold_lease(root)?;
    } else {
        let actual = store.plan_lease(id, LeaseKind::BoxRoot, new.as_str())?;
        store.hold_lease(actual)?;
    }
    Ok(())
}

/// Moves Box `row` from `old` onto the machine its fork's host was still making, once that is done;
/// one that cannot be had is released, and the setup runs in `old` from its start.
fn took_over(
    cx: &Cx,
    store: &Store,
    (row, old): (&BoxRow, &BoxHandle),
    req: &AcquireReq,
    (root, planned): (LeaseId, &BoxHandle),
) -> Result<Option<BoxHandle>, CommandError> {
    let new = match take_over(cx, store, row.id, req) {
        Ok(got) => got.handle,
        Err(e) => {
            let _ = cx.driver().release(planned);
            cx.warn(Warning {
                code: "checkpoint_failed".to_owned(),
                message: format!("could not take over the fork: {e}; setup runs from its start"),
            });
            return Ok(None);
        }
    };
    if cx.env().cancel().is_cancelled() {
        return Err(CommandError::new(ErrorKind::Failed, FORCED_CLOSE));
    }
    store.hold_lease(root)?;
    let moved = Decision::new(row.id).set(Fields {
        box_handle: Field::Set(new.clone()),
        ..Fields::default()
    });
    store.apply(moved)?;
    release_old(cx, store, row.id, old)?;
    Ok(Some(new))
}
