//! Moving a Box onto a fork of one of its kept layers: at its start, on a retry, and what a setup
//! followed again after a restart stands on.

use std::sync::atomic::Ordering;

use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::layers::{failed_layer, nearest, LayerKey};
use moor_core::seam::driver::{AcquireReq, Acquired};
use moor_core::{BoxHandle, BoxId, Warning};

use super::{Disk, Layered, RollFrom};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::disk::make_room;
use crate::store::checkpoints::{CheckpointRow, Place};
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, EventRow, Store};

/// The deepest layer kept with a place, looking first at the one above the last line.
fn deepest_kept(
    store: &Store,
    layered: &Layered,
) -> Result<Option<(usize, LayerKey, CheckpointRow, Place)>, CommandError> {
    let above = layered.finished_key.iter().map(|k| (layered.keys.len(), k));
    for (layer, key) in above.chain(layered.keys.iter().enumerate().rev()) {
        let row = store.checkpoint(&layered.home, key)?;
        if let Some((row, place)) = row.and_then(|r| r.place.clone().map(|p| (r, p))) {
            return Ok(Some((layer, key.clone(), row, place)));
        }
    }
    Ok(None)
}

/// Starts Box `id` on a fork of the deepest layer kept with a place. At the Box's base only the
/// lines above it run, and at another commit every line runs. None when no layer is kept, or when
/// the fork fails: that is recorded and warned about, and the Box is acquired and set up from
/// scratch.
pub(in super::super) fn warm(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
    layered: &mut Layered,
) -> Result<Option<Acquired>, CommandError> {
    let lines = layered.keys.len() - 1;
    let deepest = deepest_kept(store, layered)?;
    let Some((layer, key, kept, place)) = deepest else {
        make_room(cx, store, id, &layered.home)?;
        return Ok(None);
    };
    store.pin_checkpoint(id, &layered.home, &key, layer)?;
    make_room(cx, store, id, &layered.home)?;
    if !store.use_checkpoint(&layered.home, &key)? {
        return Ok(None);
    }
    let finished = layer > lines;
    let layer = layer.min(lines);
    let at = u32::try_from(layer).unwrap_or(u32::MAX);
    let began = cx.clock().now().as_millis();
    let forked = match cx.driver().fork(&kept.checkpoint, req) {
        Ok(forked) => forked,
        Err(e) => {
            let error = e.to_string();
            cx.warn(Warning {
                code: "checkpoint_failed".to_owned(),
                message: format!(
                    "could not start from setup layer {layer}: {error}; setup runs from its start"
                ),
            });
            let failed = ExtraEvent::WarmStartFailed { layer: at, error };
            store.apply(Decision::new(id).event(failed))?;
            return Ok(None);
        }
    };
    let fork_ms = u64::try_from(cx.clock().now().as_millis() - began).unwrap_or_default();
    let rerun_from = (place.base != layered.base).then(|| place.base.clone());
    match &rerun_from {
        Some(commit) => {
            layered.disk = Disk::Rerun {
                commit: commit.clone(),
            };
            layered.spent_ms.store(place.setup_ms, Ordering::Relaxed);
            layered.roll = Some(RollFrom {
                depth: kept.depth,
                root_at: kept.chain_at,
            });
        }
        None => layered.stand_on(layer, place.setup_ms),
    }
    layered.finished = finished;
    let lines = u32::try_from(lines).unwrap_or(u32::MAX);
    store.apply(Decision::new(id).event(ExtraEvent::WarmStarted {
        layer: at,
        layers: lines,
        saved_ms: place.setup_ms.saturating_sub(fork_ms),
        fork_ms,
        steps: forked.steps,
        rerun_from,
        finished,
    }))?;
    Ok(Some(Acquired {
        handle: forked.handle,
        workdir: place.workdir,
    }))
}

/// The event of the fork Box `id`'s current machine came from; none on a machine asked for since.
fn machine_fork(store: &Store, id: BoxId) -> Result<Option<EventRow>, CommandError> {
    let asked = store.newest_events_of(id, "box_requested", 1)?.pop();
    let warm = store.newest_events_of(id, "warm_started", 1)?.pop();
    let asked = asked.map(|e| e.id);
    Ok(warm.filter(|w| Some(w.id) > asked))
}

/// Marks the disk of Box `id` as a rerun when its current machine forked another commit's
/// checkpoint, whichever attempt is running on it, since that disk must never be saved as this
/// commit's. Returns whether it did.
pub(in super::super) fn stand_on_rerun(
    store: &Store,
    id: BoxId,
    layered: &mut Layered,
) -> Result<bool, CommandError> {
    let Some(fork) = machine_fork(store, id)? else {
        return Ok(false);
    };
    layered.finished = fork.payload.get("finished") == Some(&serde_json::Value::Bool(true));
    let commit = fork.payload.get("rerun_from").and_then(|c| c.as_str());
    let reruns = commit.is_some();
    if let Some(commit) = commit {
        layered.disk = Disk::Rerun {
            commit: commit.to_owned(),
        };
    }
    Ok(reruns)
}

/// Marks the disk of Box `id` as what its start forked, so a setup followed again runs what that
/// start would have.
pub(in super::super) fn stand_on_warm_start(
    store: &Store,
    id: BoxId,
    layered: &mut Layered,
) -> Result<(), CommandError> {
    if stand_on_rerun(store, id, layered)? {
        return Ok(());
    }
    let began = store
        .newest_events_of(id, "setting_up", 1)?
        .pop()
        .map(|e| e.id);
    let Some(warm) = machine_fork(store, id)?.filter(|w| Some(w.id) > began) else {
        return Ok(());
    };
    let layer = warm
        .payload
        .get("layer")
        .and_then(serde_json::Value::as_u64);
    if let Some(layer) = layer.and_then(|l| usize::try_from(l).ok()) {
        let key = layered.keys.get(layer);
        let kept = key
            .map(|k| store.checkpoint(&layered.home, k))
            .transpose()?;
        let place = kept.flatten().and_then(|r| r.place);
        let finished = layered.finished;
        layered.stand_on(layer, place.map_or(0, |p| p.setup_ms));
        layered.finished = finished;
    }
    Ok(())
}

/// The nearest checkpoint of the Box's own base at or below layer `failed`, with its layer.
fn nearest_of_own_base(
    store: &Store,
    layered: &Layered,
    failed: usize,
) -> Result<Option<(usize, CheckpointRow)>, CommandError> {
    let mut kept = Vec::new();
    for key in &layered.keys {
        let own_base = |r: &CheckpointRow| {
            let place = r.place.as_ref();
            place.is_some_and(|p| p.base == layered.base)
        };
        kept.push(store.checkpoint(&layered.home, key)?.filter(own_base));
    }
    let held = |k: &LayerKey| {
        let at = layered.keys.iter().position(|x| x == k);
        at.and_then(|i| kept.get(i)).is_some_and(Option::is_some)
    };
    let layer = nearest(&layered.keys, failed, held);
    Ok(layer.and_then(|at| Some((at, kept.into_iter().nth(at).flatten()?))))
}

/// Moves Box `row` from the machine `old` onto a new one forked from the nearest checkpoint of its
/// own base at or below its failed layer, and releases `old`. Returns the new handle; none when
/// no such checkpoint is kept or the fork fails, and the setup runs in `old` from its start.
pub(in super::super) fn resume(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    old: &BoxHandle,
    cfg: &BoxSettings,
    layered: &mut Layered,
) -> Result<Option<BoxHandle>, CommandError> {
    let failed = store
        .newest_events_of(row.id, "setup_failed", 1)?
        .pop()
        .and_then(|e| e.payload.get("line")?.as_str().map(str::to_owned))
        .map_or(cfg.setup.len(), |line| failed_layer(&cfg.setup, &line));
    make_room(cx, store, row.id, &layered.home)?;
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
    let req = layered.machine_req(cx, store, row, cfg)?;
    // Planned first, so a fork cut short is still found from its record.
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(row.id, LeaseKind::BoxRoot, planned.as_str())?;
    let spent = saved.place.as_ref().map_or(0, |p| p.setup_ms);
    let began = cx.clock().now().as_millis();
    let (new, told) = match cx.driver().fork(&saved.checkpoint, &req) {
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
    if new == planned {
        store.hold_lease(root)?;
    } else {
        let actual = store.plan_lease(row.id, LeaseKind::BoxRoot, new.as_str())?;
        store.hold_lease(actual)?;
    }
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

/// Releases the machine the Box left and drops its lease record; one that will not release
/// keeps its record and is warned about with how to release it by hand.
pub(in super::super) fn release_old(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    old: &BoxHandle,
) -> Result<(), CommandError> {
    if let Err(e) = cx.driver().release(old) {
        let by_hand = cx.driver().manual_release(old).unwrap_or_default();
        cx.warn(Warning {
            code: "release_failed".to_owned(),
            message: format!("could not release the machine the Box left: {e}. {by_hand}"),
        });
        return Ok(());
    }
    for lease in store.leases(id)? {
        if lease.kind == LeaseKind::BoxRoot && lease.key == old.as_str() {
            store.drop_lease(lease.id)?;
        }
    }
    Ok(())
}
