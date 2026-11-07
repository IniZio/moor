//! Setup in layers: on a provider that keeps checkpoints, each setup line runs on its own so the
//! Box can be checkpointed between lines. A new Box starts from the deepest checkpoint kept for
//! its layers, and a retry from the nearest one.

use std::sync::atomic::{AtomicU64, Ordering};

use moor_core::cache::CachePlan;
use moor_core::config::BoxSettings;
use moor_core::domain::layers::{chain, checkpoint_after, LayerIdentity, LayerKey};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::AcquireReq;
use moor_core::{BoxId, ProviderId};

use super::clone::SetupProxy;
use super::setup_run::run_through_proxy;
use super::{nonce, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::provider_caps;
use crate::ops::common::node::{checkpoint_home, seat_of};
use crate::ops::common::setup::{Released, SetupRun};
use crate::store::checkpoints::{CheckpointRow, Lineage, Place};
use crate::store::{BoxRow, Store};

/// What a Box's disk holds of its setup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disk {
    /// Nothing: a new machine, or one that holds no checkpoint.
    Fresh,
    /// A checkpoint of the Box's own base, whose lines are not run again.
    OnLayer {
        /// The layer it holds.
        layer: usize,
    },
    /// A checkpoint of another commit: every line runs again and nothing is saved, since the
    /// output of a line whose guard skipped it may still be that commit's.
    Rerun {
        /// The commit the disk holds.
        commit: String,
    },
}

/// The checkpoint of another commit a rerun forked, which its own save rolls its chain forward from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RollFrom {
    /// How many times its chain rolled forward.
    depth: u32,
    /// When its chain was set up from scratch.
    root_at: Ts,
}

/// A Box's setup layers: their keys, where their checkpoints live and how far its disk is.
pub(super) struct Layered {
    /// The clone's key, then one per setup line.
    keys: Vec<LayerKey>,
    /// The key of the layer above them, which holds the agent install and the profile; none when
    /// the Box has neither.
    finished_key: Option<LayerKey>,
    /// Whether the Box's disk holds that layer's steps, whichever layer it forked.
    finished: bool,
    /// What a save rolls forward from, on a rerun this start forked.
    roll: Option<RollFrom>,
    /// The oldest commit the forked disk's ignored files may come from; none on a fresh disk.
    stale_since: Option<String>,
    /// Whether this Box's saves replace layers kept already, since it was set up from scratch to
    /// get past them.
    replacing: bool,
    /// The provider a Box forks with.
    provider: ProviderId,
    /// What its checkpoints are recorded under, and the disk they fill is read by.
    home: ProviderId,
    /// Whose chain the layers make.
    lineage: Lineage,
    /// The Box the layers are for.
    id: BoxId,
    /// The commit the Box starts from.
    base: String,
    /// What the Box's disk holds.
    disk: Disk,
    /// The Box's cache plan, which its checkpoints hold the disks of.
    cache: CachePlan,
    /// Milliseconds the layers the disk holds took on the machine that set them up from scratch;
    /// each save records it, so a later Box learns what forking that layer spares it.
    spent_ms: AtomicU64,
}

impl Layered {
    /// Marks the disk as a checkpoint `kept` of `commit`, another commit, which every line runs
    /// again over and a save rolls forward from.
    fn rerun_over(&mut self, commit: &str, place: &Place, kept: &CheckpointRow) {
        self.disk = Disk::Rerun {
            commit: commit.to_owned(),
        };
        self.spent_ms.store(place.setup_ms, Ordering::Relaxed);
        self.stale_since = Some(place.stale_since.clone());
        self.roll = Some(RollFrom {
            depth: kept.depth,
            root_at: kept.chain_at,
        });
    }

    /// Marks the disk as holding layer `layer`, as `place` says it was set up.
    fn stand_on(&mut self, layer: usize, place: Option<&Place>) {
        self.disk = Disk::OnLayer { layer };
        self.finished = false;
        self.spent_ms
            .store(place.map_or(0, |p| p.setup_ms), Ordering::Relaxed);
        self.stale_since = place.map(|p| p.stale_since.clone());
    }

    /// The key of layer `layer`, counting the one above the last line.
    fn key_at(&self, layer: usize) -> Option<&LayerKey> {
        let above = self
            .finished_key
            .as_ref()
            .filter(|_| layer == self.keys.len());
        self.keys.get(layer).or(above)
    }

    /// Whether a save of this rerun may replace the layers it forked.
    pub(super) const fn may_roll(&self) -> bool {
        self.reruns() && self.roll.is_some()
    }

    /// Whether the last line's layer is kept, for this Box's lines as they are now.
    pub(super) fn top_kept(&self, store: &Store) -> bool {
        let top = self.keys.last();
        let kept = top.map(|k| store.checkpoint(&self.home, k));
        kept.is_some_and(|k| k.is_ok_and(|row| row.is_some()))
    }

    /// The index of the last line's layer.
    pub(super) const fn top(&self) -> usize {
        self.keys.len() - 1
    }

    /// The index of the layer above the last line's.
    pub(super) const fn above(&self) -> usize {
        self.keys.len()
    }

    /// Marks this Box's saves as replacing the layers kept already.
    pub(super) const fn replace_layers(&mut self) {
        self.replacing = true;
    }

    /// Retires what is left of the chain from before this Box's commit.
    pub(super) fn retire_older(&self, store: &Store) -> Result<(), CommandError> {
        store.retire_chain(&self.home, &self.lineage.chain, Some(&self.base))?;
        Ok(())
    }

    /// Whose setup a save holds, and where in its chain, at time `now`.
    fn lineage_at(&self, now: Ts) -> Lineage {
        let (depth, root_at) = match self.roll {
            Some(from) => (from.depth + 1, Some(from.root_at)),
            None if self.replacing => (0, Some(now)),
            None => (0, None),
        };
        Lineage {
            depth,
            root_at,
            ..self.lineage.clone()
        }
    }

    /// Whether the agent install and the profile are on the disk already.
    pub(super) const fn finished(&self) -> bool {
        self.finished
    }

    /// The layer whose lines are the first to run.
    const fn from(&self) -> usize {
        match self.disk {
            Disk::OnLayer { layer } => layer,
            Disk::Fresh | Disk::Rerun { .. } => 0,
        }
    }

    /// Whether every line runs again over a checkpoint of another commit.
    pub(super) const fn reruns(&self) -> bool {
        matches!(self.disk, Disk::Rerun { .. })
    }

    /// Whether the disk is a checkpoint of the Box's own base.
    pub(super) const fn holds_own_commit(&self) -> bool {
        matches!(self.disk, Disk::OnLayer { .. })
    }

    /// The oldest commit the ignored files of a forked disk may come from, when the Box is told of
    /// it: a rerun's always, and a fork of its own commit's when that disk rolled forward.
    pub(super) fn stale_from(&self) -> Option<&str> {
        let since = self.stale_since.as_deref();
        match &self.disk {
            Disk::Fresh => None,
            Disk::OnLayer { .. } => since.filter(|s| *s != self.base),
            Disk::Rerun { commit } => Some(since.unwrap_or(commit)),
        }
    }

    /// What a save records as the oldest commit its disk's ignored files may come from.
    fn saved_stale_since(&self) -> String {
        self.stale_since
            .clone()
            .unwrap_or_else(|| self.base.clone())
    }

    /// The commit a forked disk holds; none on a disk set up from scratch.
    pub(super) fn forked_commit(&self) -> Option<&str> {
        match &self.disk {
            Disk::Fresh => None,
            Disk::OnLayer { .. } => Some(&self.base),
            Disk::Rerun { commit } => Some(commit),
        }
    }

    /// What a new machine for Box `row` is asked for.
    pub(super) fn machine_req(
        &self,
        cx: &Cx,
        store: &Store,
        row: &BoxRow,
        cfg: &BoxSettings,
    ) -> Result<AcquireReq, CommandError> {
        Ok(AcquireReq {
            provider: self.provider.clone(),
            box_id: row.id,
            name: row.name.clone(),
            class: cfg.size.as_ref().map(|s| s.value.as_str().to_owned()),
            nonce: nonce(cx.rng()),
            cache: self.cache.clone(),
            pool_machine: seat_of(cx, store, row)?,
        })
    }

    /// The Box's cache plan.
    pub(super) const fn cache(&self) -> &CachePlan {
        &self.cache
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
    let identity = LayerIdentity {
        user,
        repo: &repo,
        cache: &cache.identity(),
        class: cfg.size.as_ref().map_or("", |s| s.value.as_str()),
        pinned_base: (!cfg.across_commits).then_some(base),
    };
    let keys = chain(&identity, &cfg.setup);
    let cold_asked = store
        .newest_events_of(row.id, "cold_asked", 1)
        .ok()?
        .pop()
        .is_some();
    let finished_key = keys
        .last()
        .and_then(|top| finished::key_of(cx, store, row, cfg, top));
    Some(Layered {
        finished_key,
        finished: false,
        roll: None,
        stale_since: None,
        replacing: cold_asked,
        lineage: Lineage {
            user,
            repo,
            chain: keys.first()?.clone(),
            depth: 0,
            root_at: None,
        },
        keys,
        provider,
        home,
        id: row.id,
        base: base.to_owned(),
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

mod finished;
mod fork;
mod save;

use save::save;
pub(super) use save::{after_finishing, roll_forward};

pub(super) use fork::{release_old, resume, stand_on_rerun, stand_on_warm_start, warm};
