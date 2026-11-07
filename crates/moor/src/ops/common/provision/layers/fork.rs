//! Moving a Box onto a fork of one of its kept layers: at its start, on a retry, and what a setup
//! followed again after a restart stands on.

use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::layers::{failed_layer, nearest, LayerKey};
use moor_core::seam::driver::{AcquireReq, Acquired};
use moor_core::{BoxHandle, BoxId, Warning};

use super::Layered;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::node::seat_of;
use crate::ops::common::provision::disk::make_room;
use crate::ops::common::provision::nonce;
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, Store};

/// Starts Box `id` on a fork of the deepest layer kept with a place, so only the lines above it
/// run. None when no layer is kept, or when the fork fails: that is recorded and warned about,
/// and the Box is acquired and set up from scratch.
pub(in super::super) fn warm(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
    layered: &mut Layered,
) -> Result<Option<Acquired>, CommandError> {
    let mut deepest = None;
    for (layer, key) in layered.keys.iter().enumerate().rev() {
        let row = store.checkpoint(&layered.home, key)?;
        if let Some((row, place)) = row.and_then(|r| r.place.clone().map(|p| (r, p))) {
            deepest = Some((layer, key, row.checkpoint, place));
            break;
        }
    }
    let Some((layer, key, checkpoint, place)) = deepest else {
        make_room(cx, store, id, &layered.home)?;
        return Ok(None);
    };
    store.pin_checkpoint(id, &layered.home, key, layer)?;
    make_room(cx, store, id, &layered.home)?;
    if !store.use_checkpoint(&layered.home, key)? {
        return Ok(None);
    }
    let at = u32::try_from(layer).unwrap_or(u32::MAX);
    let began = cx.clock().now().as_millis();
    let forked = match cx.driver().fork(&checkpoint, req) {
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
    layered.stand_on(layer, place.setup_ms);
    let lines = u32::try_from(layered.keys.len() - 1).unwrap_or(u32::MAX);
    store.apply(Decision::new(id).event(ExtraEvent::WarmStarted {
        layer: at,
        layers: lines,
        saved_ms: place.setup_ms.saturating_sub(fork_ms),
        fork_ms,
        steps: forked.steps,
    }))?;
    Ok(Some(Acquired {
        handle: forked.handle,
        workdir: place.workdir,
    }))
}

/// Marks the disk of Box `id` as holding the layer its start forked from, so a setup followed
/// again does not run the lines below it.
pub(in super::super) fn stand_on_warm_start(
    store: &Store,
    id: BoxId,
    layered: &mut Layered,
) -> Result<(), CommandError> {
    let began = store
        .newest_events_of(id, "setting_up", 1)?
        .pop()
        .map(|e| e.id);
    let warm = store.newest_events_of(id, "warm_started", 1)?.pop();
    let layer = warm
        .filter(|w| Some(w.id) > began)
        .and_then(|w| w.payload.get("layer")?.as_u64());
    if let Some(layer) = layer.and_then(|l| usize::try_from(l).ok()) {
        let key = layered.keys.get(layer);
        let kept = key
            .map(|k| store.checkpoint(&layered.home, k))
            .transpose()?;
        let place = kept.flatten().and_then(|r| r.place);
        layered.stand_on(layer, place.map_or(0, |p| p.setup_ms));
    }
    Ok(())
}

/// Moves Box `row` from the machine `old` onto a new one forked from the nearest checkpoint at
/// or below its failed layer, and releases `old`. Returns the new handle; none when no
/// checkpoint is kept or the fork fails, and the setup runs in `old` from its start.
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
    let mut kept = Vec::new();
    for key in &layered.keys {
        kept.push(store.checkpoint(&layered.home, key)?);
    }
    let held = |k: &LayerKey| {
        let at = layered.keys.iter().position(|x| x == k);
        at.and_then(|i| kept.get(i)).is_some_and(Option::is_some)
    };
    let Some(layer) = nearest(&layered.keys, failed, held) else {
        return Ok(None);
    };
    let (Some(Some(saved)), Some(key)) = (kept.get(layer), layered.keys.get(layer)) else {
        return Ok(None);
    };
    store.pin_checkpoint(row.id, &layered.home, key, layer)?;
    if !store.use_checkpoint(&layered.home, key)? {
        return Ok(None);
    }
    let req = AcquireReq {
        provider: layered.provider.clone(),
        box_id: row.id,
        name: row.name.clone(),
        class: cfg.size.as_ref().map(|s| s.value.as_str().to_owned()),
        nonce: nonce(cx.rng()),
        cache: layered.cache.clone(),
        pool_machine: seat_of(cx, store, row)?,
    };
    // Planned first, so a fork cut short is still found from its record.
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(row.id, LeaseKind::BoxRoot, planned.as_str())?;
    let new = match cx.driver().fork(&saved.checkpoint, &req) {
        Ok(forked) => forked.handle,
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
    store.apply(Decision::new(row.id).set(Fields {
        box_handle: Field::Set(new.clone()),
        ..Fields::default()
    }))?;
    layered.stand_on(layer, saved.place.as_ref().map_or(0, |p| p.setup_ms));
    release_old(cx, store, row, old)?;
    Ok(Some(new))
}

/// Releases the machine the Box left and drops its lease record; one that will not release
/// keeps its record and is warned about with how to release it by hand.
fn release_old(cx: &Cx, store: &Store, row: &BoxRow, old: &BoxHandle) -> Result<(), CommandError> {
    if let Err(e) = cx.driver().release(old) {
        let by_hand = cx.driver().manual_release(old).unwrap_or_default();
        cx.warn(Warning {
            code: "release_failed".to_owned(),
            message: format!("could not release the machine the Box left: {e}. {by_hand}"),
        });
        return Ok(());
    }
    for lease in store.leases(row.id)? {
        if lease.kind == LeaseKind::BoxRoot && lease.key == old.as_str() {
            store.drop_lease(lease.id)?;
        }
    }
    Ok(())
}
