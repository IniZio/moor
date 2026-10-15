//! Group signals recorded by `ScriptedProc`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use moor_core::seam::proc::{check_group, ProcError, Signal};

use super::ScriptedProc;

/// Runs when a group signal is delivered.
type Hook = Box<dyn Fn(u32, Signal) + Send + Sync>;

/// Recorded group signals and whether the groups still exist.
#[derive(Default)]
pub(super) struct Signals {
    sent: Mutex<Vec<(u32, Signal)>>,
    gone: AtomicBool,
    hook: Mutex<Option<Hook>>,
    followers: Mutex<Vec<u32>>,
}

impl ScriptedProc {
    /// Group signals sent so far, in order.
    pub(crate) fn signals(&self) -> Vec<(u32, Signal)> {
        self.signals
            .sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// From now on every group signal fails as if no such group exists.
    pub(crate) fn groups_gone(&self) -> &Self {
        self.signals.gone.store(true, Ordering::Relaxed);
        self
    }

    /// Runs `hook` whenever a group signal is delivered, as a group member reacting to it.
    pub(crate) fn on_signal(&self, hook: impl Fn(u32, Signal) + Send + Sync + 'static) -> &Self {
        *self
            .signals
            .hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(hook));
        self
    }

    /// Marks `pid` as alive but belonging to another process's group.
    pub(crate) fn follower(&self, pid: u32) -> &Self {
        self.signals
            .followers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(pid);
        self
    }

    pub(super) fn is_follower(&self, pid: u32) -> bool {
        self.signals
            .followers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&pid)
    }

    pub(super) fn send_signal(&self, pgid: u32, sig: Signal) -> Result<(), ProcError> {
        check_group(pgid)?;
        self.signals
            .sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((pgid, sig));
        if self.signals.gone.load(Ordering::Relaxed) {
            return Err(ProcError::Spawn(std::io::Error::from(
                std::io::ErrorKind::NotFound,
            )));
        }
        if let Some(hook) = self
            .signals
            .hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            hook(pgid, sig);
        }
        Ok(())
    }
}
