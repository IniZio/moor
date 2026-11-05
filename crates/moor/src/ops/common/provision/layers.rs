//! Setup in layers: on a provider that keeps checkpoints, each setup line runs on its own so the
//! Box can be checkpointed between lines. A new Box starts from the deepest checkpoint kept for
//! its layers, and a retry from the nearest one.

use std::sync::atomic::{AtomicU64, Ordering};

use moor_core::cache::CachePlan;
use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::layers::{chain, checkpoint_after, failed_layer, nearest, LayerKey};
use moor_core::seam::driver::{AcquireReq, Acquired};
use moor_core::{BoxHandle, BoxId, ProviderId, Warning};

use super::clone::SetupProxy;
use super::disk::make_room;
use super::setup_run::run_through_proxy;
use super::{nonce, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::provider_caps;
use crate::ops::common::setup::SetupRun;
use crate::store::checkpoints::{Lineage, Place};
use crate::store::lease::LeaseKind;
use crate::store::{BoxRow, Store};

/// A Box's setup layers: their keys, where their checkpoints live and how far its disk is.
pub(super) struct Layered {
    /// The clone's key, then one per setup line.
    keys: Vec<LayerKey>,
    /// The provider that holds the checkpoints.
    provider: ProviderId,
    /// Whose chain the layers make.
    lineage: Lineage,
    /// The Box the layers are for.
    id: BoxId,
    /// The layer the Box's disk already holds; its lines are not run again.
    from: usize,
    /// The Box's cache plan, which its checkpoints hold the disks of.
    cache: CachePlan,
    /// Milliseconds the layers the disk holds took to set up from scratch.
    spent_ms: AtomicU64,
}

impl Layered {
    /// Marks the disk as holding layer `layer`, which took `ms` to set up from scratch.
    fn stand_on(&mut self, layer: usize, ms: u64) {
        self.from = layer;
        self.spent_ms.store(ms, Ordering::Relaxed);
    }
}

/// The layers of Box `row` on `provider` under cache plan `cache`; none when the provider keeps
/// no checkpoints or the Box has no setup lines, no base commit, no user, or more than one repo.
pub(super) fn plan(
    cx: &Cx,
    row: &BoxRow,
    provider: &str,
    cfg: &BoxSettings,
    cache: &CachePlan,
) -> Option<Layered> {
    let repo = row.repo.as_ref().filter(|r| r.others.is_empty())?;
    let base = row.base.as_deref()?;
    let user = row.user?;
    if cfg.setup.is_empty() || !provider_caps(cx, provider).is_ok_and(|c| c.checkpoints) {
        return None;
    }
    let repo = repo.repo.to_string();
    let keys = chain(user, &repo, base, &cache.identity(), &cfg.setup);
    Some(Layered {
        lineage: Lineage {
            user,
            repo,
            chain: keys.first()?.clone(),
        },
        keys,
        provider: ProviderId::new(provider),
        id: row.id,
        from: 0,
        cache: cache.clone(),
        spent_ms: AtomicU64::new(0),
    })
}

/// Starts Box `id` on a fork of the deepest layer kept with a place, so only the lines above it
/// run. None when no layer is kept, or when the fork fails: that is recorded and warned about,
/// and the Box is acquired and set up from scratch.
pub(super) fn warm(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
    layered: &mut Layered,
) -> Result<Option<Acquired>, CommandError> {
    let mut deepest = None;
    for (layer, key) in layered.keys.iter().enumerate().rev() {
        let row = store.checkpoint(&layered.provider, key)?;
        if let Some((row, place)) = row.and_then(|r| r.place.clone().map(|p| (r, p))) {
            deepest = Some((layer, key, row.checkpoint, place));
            break;
        }
    }
    let Some((layer, key, checkpoint, place)) = deepest else {
        make_room(cx, store, id, &layered.provider)?;
        return Ok(None);
    };
    store.pin_checkpoint(id, &layered.provider, key, layer)?;
    make_room(cx, store, id, &layered.provider)?;
    if !store.use_checkpoint(&layered.provider, key)? {
        return Ok(None);
    }
    let at = u32::try_from(layer).unwrap_or(u32::MAX);
    let began = cx.clock().now().as_millis();
    let handle = match cx.driver().fork(&checkpoint, req) {
        Ok(handle) => handle,
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
    }))?;
    Ok(Some(Acquired {
        handle,
        workdir: place.workdir,
    }))
}

/// Marks the disk of Box `id` as holding the layer its start forked from, so a setup followed
/// again does not run the lines below it.
pub(super) fn stand_on_warm_start(
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
        layered.stand_on(layer, 0);
    }
    Ok(())
}

/// Checkpoints the fresh clone, which took `ms`, of a Box whose disk holds no layer yet.
pub(super) fn after_clone(cx: &Cx, store: &Store, target: &SetupTarget<'_>, ms: Option<u64>) {
    let Some(layered) = target.layered else {
        return;
    };
    if layered.from == 0 {
        layered
            .spent_ms
            .store(ms.unwrap_or_default(), Ordering::Relaxed);
        save(cx, store, target, layered, 0);
    }
}

/// Runs the setup lines the target's disk lacks one exec at a time, checkpointing after each
/// line [`checkpoint_after`] picks; stops at the first failure. Runs nothing for a target
/// without layers.
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
) -> SetupRun {
    let Some(layered) = target.layered else {
        return SetupRun::Skipped;
    };
    let total = cfg.setup.len();
    let (mut ms, mut tail) = (0, String::new());
    for (i, line) in cfg.setup.iter().enumerate().skip(layered.from) {
        let one = BoxSettings {
            setup: vec![line.clone()],
            ..cfg.clone()
        };
        let place = (
            u32::try_from(i + 1).unwrap_or(u32::MAX),
            u32::try_from(total).unwrap_or(u32::MAX),
        );
        match run_through_proxy(cx, store, id, target, &one, proxy, Some(place)) {
            SetupRun::Ok { ms: took, tail: t } => {
                ms += took;
                tail = t;
                layered.spent_ms.fetch_add(took, Ordering::Relaxed);
                if checkpoint_after(i + 1, took, total) {
                    save(cx, store, target, layered, i + 1);
                }
            }
            SetupRun::Failed {
                line,
                exit,
                ms: took,
                tail,
            } => {
                return SetupRun::Failed {
                    line,
                    exit,
                    ms: ms + took,
                    tail,
                }
            }
            SetupRun::Skipped => {}
        }
    }
    SetupRun::Ok { ms, tail }
}

/// Checkpoints `layer` of the target's Box, unless a checkpoint of it is kept already.
/// A failed checkpoint is only warned about: setup goes on, and a retry runs the layer again.
fn save(cx: &Cx, store: &Store, target: &SetupTarget<'_>, layered: &Layered, layer: usize) {
    let Some(key) = layered.keys.get(layer) else {
        return;
    };
    if store
        .use_checkpoint(&layered.provider, key)
        .unwrap_or(false)
    {
        return;
    }
    if !make_room(cx, store, layered.id, &layered.provider).unwrap_or(true) {
        cx.warn(Warning {
            code: "checkpoint_skipped".to_owned(),
            message: format!(
                "did not checkpoint setup layer {layer}: the disk is below its floor with no \
                 checkpoint left to delete"
            ),
        });
        return;
    }
    let began = cx.clock().now().as_millis();
    let taken = cx.driver().checkpoint(target.handle, key.as_str());
    let duration_ms = u64::try_from(cx.clock().now().as_millis() - began).unwrap_or_default();
    let told = ExtraEvent::CheckpointTaken {
        layer: u32::try_from(layer).unwrap_or(u32::MAX),
        key: key.as_str().to_owned(),
        checkpoint: taken.as_ref().ok().map(|c| c.id.clone()),
        duration_ms,
        error: taken.as_ref().err().map(ToString::to_string),
    };
    if let Err(e) = store.apply(Decision::new(layered.id).event(told)) {
        cx.warn(Warning {
            code: "checkpoint_unrecorded".to_owned(),
            message: format!("could not record the checkpoint of setup layer {layer}: {e}"),
        });
    }
    let saved = taken.map_err(|e| e.to_string()).and_then(|made| {
        let place = Place {
            workdir: target.workdir.clone(),
            setup_ms: layered.spent_ms.load(Ordering::Relaxed),
        };
        store
            .record_checkpoint(&layered.provider, key, &layered.lineage, &made)
            .and_then(|()| store.place_checkpoint(&layered.provider, key, &place))
            .and_then(|()| store.pin_checkpoint(layered.id, &layered.provider, key, layer))
            .map_err(|e| e.to_string())
    });
    if let Err(e) = saved {
        cx.warn(Warning {
            code: "checkpoint_failed".to_owned(),
            message: format!(
                "could not checkpoint setup layer {layer}: {e}; a retry runs it again"
            ),
        });
    }
}

/// Moves Box `row` from the machine `old` onto a new one forked from the nearest checkpoint at
/// or below its failed layer, and releases `old`. Returns the new handle; none when no
/// checkpoint is kept or the fork fails, and the setup runs in `old` from its start.
pub(super) fn resume(
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
    make_room(cx, store, row.id, &layered.provider)?;
    let mut kept = Vec::new();
    for key in &layered.keys {
        kept.push(store.checkpoint(&layered.provider, key)?);
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
    store.pin_checkpoint(row.id, &layered.provider, key, layer)?;
    if !store.use_checkpoint(&layered.provider, key)? {
        return Ok(None);
    }
    let req = AcquireReq {
        provider: layered.provider.clone(),
        box_id: row.id,
        name: row.name.clone(),
        class: Some(cfg.size.value.as_str().to_owned()),
        nonce: nonce(cx.rng()),
        cache: layered.cache.clone(),
        pool_machine: None,
    };
    // Planned first, so a fork cut short is still found from its record.
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(row.id, LeaseKind::BoxRoot, planned.as_str())?;
    let new = match cx.driver().fork(&saved.checkpoint, &req) {
        Ok(new) => new,
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
