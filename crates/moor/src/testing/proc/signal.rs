//! Group signals recorded by `ScriptedProc`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use moor_core::seam::proc::{check_group, ProcError, Signal};

use super::ScriptedProc;

/// Recorded group signals and whether the groups still exist.
#[derive(Default)]
pub(super) struct Signals {
    sent: Mutex<Vec<(u32, Signal)>>,
    gone: AtomicBool,
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
        Ok(())
    }
}
