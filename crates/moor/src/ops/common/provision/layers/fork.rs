//! Moving a Box onto a fork of one of its kept layers: at its start, on a retry, and what a setup
//! followed again after a restart stands on.

use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::layers::{chain_past_limits, failed_layer, nearest, LayerKey};
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError, Forked};
use moor_core::{BoxHandle, BoxId, Warning};

use super::{Disk, Layered};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::disk::make_room;
use crate::ops::common::provision::progress::{goes_on_after, take_over};
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

/// Marks the disk as the fork of `kept`, layer `layer`, and returns the commit it holds when that
/// is not the Box's, so its lines run again and a save rolls the chain forward.
fn stand_on_fork(
    layered: &mut Layered,
    layer: usize,
    place: &Place,
    kept: &CheckpointRow,
) -> Option<String> {
    let rerun_from = (place.base != layered.base).then(|| place.base.clone());
    match &rerun_from {
        Some(commit) => layered.rerun_over(commit, place, kept),
        None => layered.stand_on(layer, Some(place)),
    }
    rerun_from
}

/// Retires the chain of `kept`, deepest of Box `id`'s layers, when it is past its limits or no one
/// can tell how old its ignored files are, and says so; whether it was.
fn chain_expired(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    layered: &Layered,
    (layer, kept): (usize, &CheckpointRow),
) -> Result<bool, CommandError> {
    let age_ms = store.now().as_millis() - kept.chain_at.as_millis();
    let unknown = kept.place.as_ref().is_some_and(|p| p.stale_since.is_none());
    let why = chain_past_limits(age_ms, kept.depth).or_else(|| {
        let why = "its disk rolled forward before moor kept the oldest commit it may hold";
        unknown.then(|| why.to_owned())
    });
    let Some(why) = why else {
        return Ok(false);
    };
    store.retire_chain(&layered.home, &layered.lineage.chain, None)?;
    make_room(cx, store, id, &layered.home)?;
    cx.warn(Warning {
        code: "checkpoint_chain_expired".to_owned(),
        message: format!("setup runs from its start: {why}"),
    });
    let layer = u32::try_from(layer).unwrap_or(u32::MAX);
    let told = ExtraEvent::WarmStartFailed { layer, error: why };
    store.apply(Decision::new(id).event(told))?;
    Ok(true)
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
) -> Result<Option<Result<Acquired, DriverError>>, CommandError> {
    let lines = layered.keys.len() - 1;
    let deepest = deepest_kept(store, layered)?.filter(|_| !layered.replacing);
    let Some((layer, key, kept, place)) = deepest else {
        make_room(cx, store, id, &layered.home)?;
        return Ok(None);
    };
    if chain_expired(cx, store, id, layered, (layer, &kept))? {
        return Ok(None);
    }
    store.pin_checkpoint(id, &layered.home, &key, layer)?;
    make_room(cx, store, id, &layered.home)?;
    if !store.use_checkpoint(&layered.home, &key)? {
        return Ok(None);
    }
    let finished = layer > lines;
    let layer = layer.min(lines);
    let at = u32::try_from(layer).unwrap_or(u32::MAX);
    let began = cx.clock().now().as_millis();
    let forked = match fork_or_take_over(cx, store, id, req, &kept.checkpoint) {
        // A forced close ended it, so the Box gets no other machine.
        Err(e) if cx.env().cancel().is_cancelled() => return Ok(Some(Err(e))),
        Ok(forked) => forked,
        Err(e @ DriverError::Transport { .. }) if goes_on_after(cx, req, &e) => {
            return Ok(Some(Err(e)))
        }
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
    let rerun_from = stand_on_fork(layered, layer, &place, &kept);
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
    Ok(Some(Ok(Acquired {
        handle: forked.handle,
        workdir: place.workdir,
    })))
}

/// Forks `checkpoint` for Box `id`; a fork cut off by a lost link goes on at its host and is taken
/// over.
fn fork_or_take_over(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
    checkpoint: &str,
) -> Result<Forked, DriverError> {
    match cx.driver().fork(checkpoint, req) {
        Err(e) if goes_on_after(cx, req, &e) => take_over(cx, store, id, req).map(|got| Forked {
            handle: got.handle,
            steps: Vec::new(),
        }),
        forked => forked,
    }
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
        layered.stand_on(layer, place.as_ref());
        layered.finished = finished;
    }
    Ok(())
}

/// The nearest checkpoint of the Box's own base at or below layer `failed`, with its layer.
pub(super) fn nearest_of_own_base(
    store: &Store,
    layered: &Layered,
    failed: usize,
) -> Result<Option<(usize, CheckpointRow)>, CommandError> {
    let mut kept = Vec::new();
    for key in &layered.keys {
        let own_base = |r: &CheckpointRow| {
            let place = r.place.as_ref();
            place.is_some_and(|p| p.base == layered.base && p.stale_since.is_some())
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

/// The layer whose line Box `row` failed last; the top when it names none.
pub(super) fn failed_at(
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
) -> Result<usize, CommandError> {
    let failed = store.newest_events_of(row.id, "setup_failed", 1)?.pop();
    let line = failed.and_then(|e| e.payload.get("line")?.as_str().map(str::to_owned));
    Ok(line.map_or(cfg.setup.len(), |line| failed_layer(&cfg.setup, &line)))
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
