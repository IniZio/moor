//! The fake's checkpoints, and the per-command exec knobs that drive layered setup.

use std::sync::Arc;

use moor_core::seam::driver::{
    AcquireReq, Checkpoint, CheckpointDisk, DriverError, ExecOutcome, ExecSpec,
};
use moor_core::BoxHandle;

use super::script::ExecHook;
use super::{FakeDriver, State};

/// Whether the fake offers checkpoints, the ones it saved, and the per-command exec knobs.
#[derive(Default)]
pub(super) struct LayerKnobs {
    /// The capability.
    pub(super) on: bool,
    /// Every checkpoint saved and not deleted, oldest first: id, Box and name.
    made: Vec<(String, String, String)>,
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
    /// The error the next fork ends with, once.
    fork_fails: Option<DriverError>,
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

    /// Ends the next fork with `e`, once.
    pub(crate) fn fork_fails_once(self, e: DriverError) -> Self {
        self.st().layers.fork_fails = Some(e);
        self
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

    /// The ids of the checkpoints kept, oldest first.
    pub(crate) fn checkpoint_ids(&self) -> Vec<String> {
        self.st().layers.made.iter().map(|m| m.0.clone()).collect()
    }

    /// The names of the checkpoints saved, oldest first.
    pub(crate) fn checkpoint_names(&self) -> Vec<String> {
        self.st().layers.made.iter().map(|m| m.2.clone()).collect()
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
        s.layers.saved += 1;
        let id = format!("ckpt-{}", s.layers.saved);
        let made = (id.clone(), handle.as_str().to_owned(), name.to_owned());
        s.layers.made.push(made);
        let each = s.layers.each_mib;
        if let Some(d) = s.layers.disk.as_mut() {
            d.free_mib = d.free_mib.saturating_sub(each);
        }
        let bytes = if each == 0 { 1024 } else { each * 1024 * 1024 };
        Ok(Checkpoint {
            id,
            bytes: Some(bytes),
        })
    }

    /// Deletes a kept checkpoint, freeing its share of the disk.
    pub(super) fn deleted(&self, id: &str) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("delete-checkpoint {id}"));
        let at = s
            .layers
            .made
            .iter()
            .position(|m| m.0 == id)
            .ok_or(DriverError::NotFound)?;
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
    pub(super) fn forked(&self, id: &str, req: &AcquireReq) -> Result<BoxHandle, DriverError> {
        let mut s = self.st();
        s.note(format!("fork {id}"));
        if let Some(e) = s.layers.fork_fails.take() {
            return Err(e);
        }
        if !s.layers.made.iter().any(|m| m.0 == id) {
            return Err(DriverError::NotFound);
        }
        s.next += 1;
        let handle = format!("box-{}", s.next);
        s.live.push(handle.clone());
        s.providers.push((handle.clone(), req.provider.clone()));
        Ok(BoxHandle::new(handle))
    }
}
