//! The per-Task proxy supervisor run by `moor proxy serve <task>`.
//!
//! It claims the lease on the Task's proxy row, starts the nono proxy, opens the reverse
//! tunnel into the Box and marks the row ready. It then heartbeats until the Task closes, the
//! lease is lost, the tunnel dies or a signal arrives.

#[cfg_attr(not(test), allow(dead_code, reason = "removed in the next change"))]
mod creds;
mod listen;
mod run;
#[cfg(test)]
mod tests;
mod tunnel;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::forge::Forge;
use moor_core::seam::rng::Rng;
use moor_core::TaskId;

use super::nono::capture::CredsFn;
use crate::store::Store;
use run::{Running, Stop};

/// How long a lease stays valid without a heartbeat.
pub(crate) const LEASE: Duration = Duration::from_secs(30);
/// Pause between heartbeats.
pub(crate) const TICK: Duration = Duration::from_secs(2);

/// Shared line sink; the lock keeps each line whole.
pub(crate) type Sink = Arc<Mutex<dyn Write + Send>>;

/// Everything the supervisor needs from the outside.
pub(crate) struct Deps<'a> {
    /// Task and proxy rows.
    pub(crate) store: &'a Store,
    /// Opens the reverse tunnel into the Box.
    pub(crate) driver: &'a dyn Driver,
    /// Forge the Task repo lives on.
    pub(crate) forge: Arc<dyn Forge>,
    /// Source of time and sleeping.
    pub(crate) clock: Arc<dyn Clock>,
    /// Source of the lease owner token.
    pub(crate) rng: &'a dyn Rng,
    /// Yields the secret behind a named credential.
    pub(crate) creds: CredsFn,
    /// Extra upstream root PEM the proxy trusts; `None` in production.
    pub(crate) upstream_ca: Option<PathBuf>,
    /// Directory holding the proxy logs, CA and spec.
    pub(crate) state_dir: &'a Path,
    /// Host boot id recorded in the row at readiness; `None` where the host has none.
    pub(crate) boot_id: Option<String>,
    /// Set when a signal asks the supervisor to stop.
    pub(crate) stop: Arc<AtomicBool>,
    /// Where status lines go.
    pub(crate) stderr: Sink,
}

/// Supervises the proxy of `task` until it must stop; returns the process exit code.
pub(crate) fn serve(deps: &Deps<'_>, task: TaskId) -> u8 {
    match start(deps, task) {
        Err(code) => code,
        Ok(running) => loop {
            if deps.stop.load(Ordering::Acquire) {
                return running.finish(Stop::Signal);
            }
            if let Some(stop) = running.tick() {
                return running.finish(stop);
            }
            deps.clock.sleep(TICK);
        },
    }
}

/// Claims the lease and brings the listener and tunnel up. `Err` carries the exit code.
fn start<'d>(deps: &'d Deps<'d>, task: TaskId) -> Result<Running<'d>, u8> {
    run::begin(deps, task)
}
