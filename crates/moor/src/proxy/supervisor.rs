//! The per-Box proxy supervisor run by `moor proxy serve <box>`.
//!
//! It claims the lease on the Box's proxy row, starts the nono proxy, opens the reverse
//! tunnel into the machine and marks the row ready. It then heartbeats until the Box closes, the
//! lease is lost, the tunnel or the front socket is lost for good, or a signal arrives.

mod bind;
mod listen;
mod machine;
mod park;
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
use moor_core::seam::proc::Proc;
use moor_core::seam::rng::Rng;
use moor_core::BoxId;

use super::nono::capture::SecretsFn;
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
    /// Box and proxy rows.
    pub(crate) store: &'a Store,
    /// Opens the reverse tunnel into the Box.
    pub(crate) driver: &'a dyn Driver,
    /// Starts the ssh child that serves the attached machine's forwards.
    pub(crate) proc: &'a dyn Proc,
    /// Source of time and sleeping.
    pub(crate) clock: Arc<dyn Clock>,
    /// Source of the lease owner token.
    pub(crate) rng: &'a dyn Rng,
    /// Reads a user's secret from the store, on every request that needs one.
    pub(crate) secrets: SecretsFn,
    /// Extra upstream root PEM the proxy trusts; `None` in production.
    pub(crate) upstream_ca: Option<PathBuf>,
    /// Address the GraphQL filter dials instead of resolving the GitHub API host; `None` in production.
    pub(crate) upstream_dial: Option<std::net::SocketAddr>,
    /// Loopback addresses the egress guard lets through; empty in production.
    pub(crate) guard_exempt: Vec<std::net::SocketAddr>,
    /// Directory holding the proxy logs, CA and spec.
    pub(crate) state_dir: &'a Path,
    /// moor's private socket directory, where the front listens.
    pub(crate) sockets: &'a Path,
    /// The uid that must own `sockets`: the uid moor runs as.
    pub(crate) owner: u32,
    /// Host boot id recorded in the row at readiness; `None` where the host has none.
    pub(crate) boot_id: Option<String>,
    /// Set when a signal asks the supervisor to stop.
    pub(crate) stop: Arc<AtomicBool>,
    /// Where status lines go.
    pub(crate) stderr: Sink,
}

/// Supervises the proxy of `box_id` until it must stop; returns the process exit code.
pub(crate) fn serve(deps: &Deps<'_>, box_id: BoxId) -> u8 {
    match start(deps, box_id) {
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
fn start<'d>(deps: &'d Deps<'d>, box_id: BoxId) -> Result<Running<'d>, u8> {
    run::begin(deps, box_id)
}
