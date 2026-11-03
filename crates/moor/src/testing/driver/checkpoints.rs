//! The fake's checkpoints, and the per-command exec knobs that drive layered setup.

use std::sync::Arc;

use moor_core::seam::driver::{AcquireReq, Checkpoint, DriverError, ExecOutcome, ExecSpec};
use moor_core::BoxHandle;

use super::script::ExecHook;
use super::{FakeDriver, State};

/// Whether the fake offers checkpoints, the ones it saved, and the per-command exec knobs.
#[derive(Default)]
pub(super) struct LayerKnobs {
    /// The capability.
    pub(super) on: bool,
    /// Every checkpoint saved, oldest first: id, Box and name.
    made: Vec<(String, String, String)>,
    /// Outcomes that end the next `exec` whose argv mentions the needle, once each.
    ends_once: Vec<(String, ExecOutcome)>,
    /// Hooks run before any `exec` whose argv mentions the needle, outside the lock.
    hooks: Vec<(String, Arc<ExecHook>)>,
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
        let id = format!("ckpt-{}", s.layers.made.len() + 1);
        let made = (id.clone(), handle.as_str().to_owned(), name.to_owned());
        s.layers.made.push(made);
        Ok(Checkpoint {
            id,
            bytes: Some(1024),
        })
    }

    /// A new live Box from a saved checkpoint, under the next planned handle.
    pub(super) fn forked(&self, id: &str, req: &AcquireReq) -> Result<BoxHandle, DriverError> {
        let mut s = self.st();
        s.note(format!("fork {id}"));
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
