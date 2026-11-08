//! The fake's checkpoints, and the per-command exec knobs that drive layered setup.

use std::sync::Arc;

use moor_core::domain::event::ProviderStep;
use moor_core::seam::driver::{
    AcquireReq, Checkpoint, CheckpointDisk, DriverError, ExecOutcome, ExecSpec, Forked,
};
use moor_core::BoxHandle;

use super::script::ExecHook;
use super::{FakeDriver, State};

/// Whether the fake offers checkpoints, the ones it saved, and the per-command exec knobs.
#[derive(Default)]
pub(super) struct LayerKnobs {
    /// The capability.
    pub(super) on: bool,
    /// Every checkpoint saved and not deleted, oldest first.
    made: Vec<Saved>,
    /// Whether each checkpoint is a delta over the one its Box last saved or started from.
    deltas: bool,
    /// The checkpoint each live Box's disk is a delta over, by handle.
    bases: Vec<(String, String)>,
    /// How many checkpoints were ever saved, which numbers the next.
    saved: usize,
    /// The disk the checkpoints live on, when the fake reports one.
    disk: Option<CheckpointDisk>,
    /// MiB each checkpoint takes of that disk.
    each_mib: u64,
    /// Outcomes that end the next `exec` whose argv mentions the needle, once each.
    ends_once: Vec<(String, ExecOutcome)>,
    /// Hooks run before any `exec` whose argv mentions the needle, outside the lock.
    hooks: Vec<(String, Arc<ExecHook>)>,
    /// Run before every `copy_out`, outside the lock.
    copy_out_hook: Option<Arc<ExecHook>>,
    /// The error the next fork ends with, once.
    fork_fails: Option<DriverError>,
    /// Run as the next fork starts, outside the lock, once; a panic there is a process dying.
    during_fork: Option<Arc<ExecHook>>,
    /// What every fork reports it spent its time on.
    fork_steps: Vec<ProviderStep>,
    /// The error the next checkpoint ends with, once.
    checkpoint_fails: Option<DriverError>,
    /// Whether every delete fails, leaving the checkpoint on the disk.
    deletes_fail: bool,
    /// The machine each fork was asked for, in order.
    fork_seats: Vec<Option<i64>>,
}

/// A checkpoint the fake keeps.
struct Saved {
    /// Its id.
    id: String,
    /// The name it was saved under.
    name: String,
    /// The checkpoint it is a delta over, when deltas are on.
    parent: Option<String>,
}

impl State {
    /// The outcome set up to end the next exec whose argv is `argv`, used up once served.
    pub(super) fn end_once(&mut self, argv: &str) -> Option<ExecOutcome> {
        let at = self
            .layers
            .ends_once
            .iter()
            .position(|(n, _)| argv.contains(n.as_str()))?;
        Some(self.layers.ends_once.remove(at).1)
    }
}

impl FakeDriver {
    /// Offers checkpoints.
    pub(crate) fn with_checkpoints(self) -> Self {
        self.st().layers.on = true;
        self
    }

    /// Offers checkpoints from now on, without consuming the driver.
    pub(crate) fn offer_checkpoints(&self) {
        self.st().layers.on = true;
    }

    /// Saves each checkpoint as a delta over the one its Box stands on, which refuses to be
    /// deleted while a delta over it is kept.
    pub(crate) fn with_delta_checkpoints(self) -> Self {
        self.st().layers.deltas = true;
        self
    }

    /// Has every fork report `steps`.
    pub(crate) fn with_fork_steps(self, steps: Vec<ProviderStep>) -> Self {
        self.st().layers.fork_steps = steps;
        self
    }

    /// Runs `f` as the next fork starts, on a driver already in use.
    pub(crate) fn during_next_fork(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().layers.during_fork = Some(Arc::new(f));
    }

    /// Ends the next fork with `e`, on a driver already in use.
    pub(crate) fn fork_fails_next(&self, e: DriverError) {
        self.st().layers.fork_fails = Some(e);
    }

    /// Ends the next fork with `e`, once.
    pub(crate) fn fork_fails_once(self, e: DriverError) -> Self {
        self.st().layers.fork_fails = Some(e);
        self
    }

    /// Ends the next checkpoint with `e`, once.
    pub(crate) fn checkpoint_fails_once(self, e: DriverError) -> Self {
        self.st().layers.checkpoint_fails = Some(e);
        self
    }

    /// Makes every checkpoint delete fail, or succeed again.
    pub(crate) fn set_deletes_fail(&self, fail: bool) {
        self.st().layers.deletes_fail = fail;
    }

    /// Ends the next `exec` whose argv mentions `needle` with `outcome`, once.
    pub(crate) fn exec_ends_once_when(self, needle: &str, outcome: ExecOutcome) -> Self {
        self.st()
            .layers
            .ends_once
            .push((needle.to_owned(), outcome));
        self
    }

    /// Runs `f` before every `exec` whose argv mentions `needle`, outside the Driver's lock.
    pub(crate) fn on_exec_of(&self, needle: &str, f: impl Fn() + Send + Sync + 'static) {
        self.st()
            .layers
            .hooks
            .push((needle.to_owned(), Arc::new(f)));
    }

    /// Reports a checkpoint disk of `total_mib` with `free_mib` free, each checkpoint taking
    /// `each_mib` of it.
    pub(crate) fn with_checkpoint_disk(self, total_mib: u64, free_mib: u64, each_mib: u64) -> Self {
        {
            let mut s = self.st();
            s.layers.disk = Some(CheckpointDisk {
                free_mib,
                total_mib,
            });
            s.layers.each_mib = each_mib;
        }
        self
    }

    /// Sets the free MiB of the checkpoint disk, as if something else used or freed it.
    pub(crate) fn set_free_mib(&self, free_mib: u64) {
        let mut s = self.st();
        if let Some(d) = s.layers.disk.as_mut() {
            d.free_mib = free_mib;
        }
        drop(s);
    }

    /// The machine each fork was asked for, in order.
    pub(crate) fn fork_seats(&self) -> Vec<Option<i64>> {
        self.st().layers.fork_seats.clone()
    }

    /// The ids of the checkpoints kept, oldest first.
    pub(crate) fn checkpoint_ids(&self) -> Vec<String> {
        self.st().layers.made.iter().map(|m| m.id.clone()).collect()
    }

    /// The names of the checkpoints saved, oldest first.
    pub(crate) fn checkpoint_names(&self) -> Vec<String> {
        self.st()
            .layers
            .made
            .iter()
            .map(|m| m.name.clone())
            .collect()
    }

    /// Runs the hook set up for every exec, then those set up for `spec`'s argv.
    pub(super) fn run_exec_hooks(&self, spec: &ExecSpec) {
        let argv = spec.argv.join(" ");
        let s = self.st();
        let named = s
            .layers
            .hooks
            .iter()
            .filter(|(n, _)| argv.contains(n.as_str()));
        let hooks: Vec<_> = s
            .on_exec
            .iter()
            .chain(named.map(|(_, h)| h))
            .cloned()
            .collect();
        drop(s);
        for hook in hooks {
            hook();
        }
    }

    /// Runs the hook set up for every `copy_out`.
    pub(super) fn run_copy_out_hook(&self) {
        let hook = self.st().layers.copy_out_hook.clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Runs `f` before every `copy_out`, outside the Driver's lock.
    pub(crate) fn on_copy_out(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().layers.copy_out_hook = Some(Arc::new(f));
    }

    /// Saves checkpoint `ckpt-N` of a live Box.
    pub(super) fn checkpointed(
        &self,
        handle: &BoxHandle,
        name: &str,
    ) -> Result<Checkpoint, DriverError> {
        let mut s = self.st();
        s.note(format!("checkpoint {}", handle.as_str()));
        if !s.layers.on {
            return Err(DriverError::Unsupported("checkpoints"));
        }
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.layers.checkpoint_fails.take() {
            return Err(e);
        }
        s.layers.saved += 1;
        let id = format!("ckpt-{}", s.layers.saved);
        if let Some(trees) = &s.trees {
            trees.saved(handle.as_str(), &id)?;
        }
        let parent = s
            .layers
            .base_of(handle.as_str())
            .filter(|_| s.layers.deltas);
        s.layers.stand(handle.as_str(), &id);
        s.layers.made.push(Saved {
            id: id.clone(),
            name: name.to_owned(),
            parent: parent.clone(),
        });
        let each = s.layers.each_mib;
        if let Some(d) = s.layers.disk.as_mut() {
            d.free_mib = d.free_mib.saturating_sub(each);
        }
        let bytes = if each == 0 { 1024 } else { each * 1024 * 1024 };
        Ok(Checkpoint {
            id,
            bytes: Some(bytes),
            parent,
        })
    }

    /// Deletes a kept checkpoint, freeing its share of the disk.
    pub(super) fn deleted(&self, id: &str) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("delete-checkpoint {id}"));
        if s.layers.deletes_fail {
            return Err(DriverError::Unavailable("disk busy".to_owned()));
        }
        let at = s
            .layers
            .made
            .iter()
            .position(|m| m.id == id)
            .ok_or(DriverError::NotFound)?;
        let over: Vec<&str> = s
            .layers
            .made
            .iter()
            .filter(|m| m.parent.as_deref() == Some(id))
            .map(|m| m.id.as_str())
            .collect();
        if !over.is_empty() {
            return Err(DriverError::Failed {
                summary: format!(
                    "cannot delete checkpoint {id}: checkpoints {} are deltas over it; delete \
                     them first",
                    over.join(", ")
                ),
            });
        }
        s.layers.made.remove(at);
        let each = s.layers.each_mib;
        if let Some(d) = s.layers.disk.as_mut() {
            d.free_mib = d.free_mib.saturating_add(each);
        }
        Ok(())
    }

    /// The checkpoint disk, when the fake offers checkpoints and reports one.
    pub(super) fn disk(&self) -> Option<CheckpointDisk> {
        let s = self.st();
        s.layers.disk.filter(|_| s.layers.on)
    }

    /// A new live Box from a saved checkpoint, under the next planned handle.
    pub(super) fn forked(&self, id: &str, req: &AcquireReq) -> Result<Forked, DriverError> {
        let during = self.st().layers.during_fork.take();
        if let Some(during) = during {
            during();
        }
        let mut s = self.st();
        s.note(format!("fork {id}"));
        let seat = req.pool_machine.as_ref().map(|s| s.machine);
        s.layers.fork_seats.push(seat);
        s.cache.forks.push(req.cache.clone());
        if let Some(e) = s.layers.fork_fails.take() {
            return Err(e);
        }
        if !s.layers.made.iter().any(|m| m.id == id) {
            return Err(DriverError::NotFound);
        }
        s.next += 1;
        let handle = format!("box-{}", s.next);
        if let Some(trees) = &s.trees {
            trees.forked(id, &handle)?;
        }
        s.layers.stand(&handle, id);
        s.live.push(handle.clone());
        s.providers.push((handle.clone(), req.provider.clone()));
        s.cache.classes.push(req.class.clone());
        Ok(Forked {
            handle: BoxHandle::new(handle),
            steps: s.layers.fork_steps.clone(),
        })
    }
}

impl LayerKnobs {
    /// The checkpoint the Box behind `handle` stands on.
    fn base_of(&self, handle: &str) -> Option<String> {
        let base = self.bases.iter().find(|(h, _)| h == handle);
        base.map(|(_, id)| id.clone())
    }

    /// Marks the Box behind `handle` as standing on checkpoint `id`.
    fn stand(&mut self, handle: &str, id: &str) {
        self.bases.retain(|(h, _)| h != handle);
        self.bases.push((handle.to_owned(), id.to_owned()));
    }
}
