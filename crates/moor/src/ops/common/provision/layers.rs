//! Setup in layers: on a provider that keeps checkpoints, each setup line runs on its own so the
//! Box can be checkpointed between lines. A new Box starts from the deepest checkpoint kept for
//! its layers, and a retry from the nearest one.

use std::sync::atomic::{AtomicU64, Ordering};

use moor_core::cache::CachePlan;
use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::layers::{chain, checkpoint_after, LayerKey};
use moor_core::{BoxId, ProviderId, Warning};

use super::clone::SetupProxy;
use super::disk::make_room;
use super::setup_run::run_through_proxy;
use super::SetupTarget;
use crate::cx::Cx;
use crate::ops::common::facts::provider_caps;
use crate::ops::common::node::checkpoint_home;
use crate::ops::common::setup::{Released, SetupRun};
use crate::store::checkpoints::{Lineage, Place};
use crate::store::{BoxRow, Store};

/// What a Box's disk holds of its setup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disk {
    /// Nothing: a new machine, or one that holds no checkpoint.
    Fresh,
    /// A checkpoint, whose lines are not run again.
    OnLayer {
        /// The layer it holds.
        layer: usize,
    },
}

/// A Box's setup layers: their keys, where their checkpoints live and how far its disk is.
pub(super) struct Layered {
    /// The clone's key, then one per setup line.
    keys: Vec<LayerKey>,
    /// The provider a Box forks with.
    provider: ProviderId,
    /// What its checkpoints are recorded under, and the disk they fill is read by.
    home: ProviderId,
    /// Whose chain the layers make.
    lineage: Lineage,
    /// The Box the layers are for.
    id: BoxId,
    /// What the Box's disk holds.
    disk: Disk,
    /// The Box's cache plan, which its checkpoints hold the disks of.
    cache: CachePlan,
    /// Milliseconds the layers the disk holds took on the machine that set them up from scratch;
    /// each save records it, so a later Box learns what forking that layer spares it.
    spent_ms: AtomicU64,
}

impl Layered {
    /// Marks the disk as holding layer `layer`, which took `ms` to set up from scratch.
    fn stand_on(&mut self, layer: usize, ms: u64) {
        self.disk = Disk::OnLayer { layer };
        self.spent_ms.store(ms, Ordering::Relaxed);
    }

    /// The layer whose lines are the first to run.
    const fn from(&self) -> usize {
        match self.disk {
            Disk::OnLayer { layer } => layer,
            Disk::Fresh => 0,
        }
    }
}

/// The layers of Box `row` on `provider` under cache plan `cache`; none when the provider keeps
/// no checkpoints or the Box has no setup lines, no base commit, no user, or more than one repo.
pub(super) fn plan(
    cx: &Cx,
    store: &Store,
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
    let provider = ProviderId::new(provider);
    let home = checkpoint_home(cx, store, row, &provider, user)?;
    let repo = repo.repo.to_string();
    let keys = chain(user, &repo, base, &cache.identity(), &cfg.setup);
    Some(Layered {
        lineage: Lineage {
            user,
            repo,
            chain: keys.first()?.clone(),
        },
        keys,
        provider,
        home,
        id: row.id,
        disk: Disk::Fresh,
        cache: cache.clone(),
        spent_ms: AtomicU64::new(0),
    })
}

/// Checkpoints the fresh clone, which took `ms`, of a Box whose disk holds no layer yet.
pub(super) fn after_clone(cx: &Cx, store: &Store, target: &SetupTarget<'_>, ms: Option<u64>) {
    let Some(layered) = target.layered else {
        return;
    };
    if layered.disk == Disk::Fresh {
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
) -> Result<SetupRun, Released> {
    let Some(layered) = target.layered else {
        return Ok(SetupRun::Skipped);
    };
    let total = cfg.setup.len();
    let (mut ms, mut tail) = (0, String::new());
    for (i, line) in cfg.setup.iter().enumerate().skip(layered.from()) {
        let one = BoxSettings {
            setup: vec![line.clone()],
            ..cfg.clone()
        };
        let place = (
            u32::try_from(i + 1).unwrap_or(u32::MAX),
            u32::try_from(total).unwrap_or(u32::MAX),
        );
        match run_through_proxy(cx, store, id, target, &one, proxy, Some(place))? {
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
                return Ok(SetupRun::Failed {
                    line,
                    exit,
                    ms: ms + took,
                    tail,
                })
            }
            SetupRun::Skipped => {}
        }
    }
    Ok(SetupRun::Ok { ms, tail })
}

/// Checkpoints `layer` of the target's Box, unless a checkpoint of it is kept already.
/// A failed checkpoint is only warned about: setup goes on, and a retry runs the layer again.
fn save(cx: &Cx, store: &Store, target: &SetupTarget<'_>, layered: &Layered, layer: usize) {
    let Some(key) = layered.keys.get(layer) else {
        return;
    };
    if store.use_checkpoint(&layered.home, key).unwrap_or(false) {
        return;
    }
    if !make_room(cx, store, layered.id, &layered.home).unwrap_or(true) {
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
            .record_checkpoint(&layered.home, key, &layered.lineage, &made)
            .and_then(|()| store.place_checkpoint(&layered.home, key, &place))
            .and_then(|()| store.pin_checkpoint(layered.id, &layered.home, key, layer))
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

mod fork;

pub(super) use fork::{resume, stand_on_warm_start, warm};
