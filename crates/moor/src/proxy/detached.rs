//! Proxy host that runs each Task's supervisor as a detached `moor proxy serve` process.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use moor_core::seam::clock::Clock;
use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Signal, Tool};
use moor_core::seam::proxy_host::{
    ProxyEndpoint, ProxyHost, ProxyHostError, ProxySpec, ProxyStatus,
};
use moor_core::seam::rng::Rng;
use moor_core::{BoxId, Secret};

use crate::proxy::nono::ca::load_or_create;
use crate::store::{ProxyRow, Reserve, Store, StoreError, STORE_FILE};

pub(crate) use super::supervisor::LEASE;
pub(crate) use spec::{read_spec, write_spec};

mod spec;

/// How far ahead of the clock a heartbeat may be and still count as live.
const SKEW: Duration = Duration::from_secs(5);

/// Pause between two looks at the proxy row.
pub(crate) const POLL: Duration = Duration::from_millis(200);

/// How long to wait for the supervisor to become ready.
pub(crate) const WAIT: Duration = Duration::from_secs(30);

/// Log lines kept when a start fails.
pub(crate) const TAIL_LINES: usize = 20;

/// How long close waits for a signalled supervisor to remove its row.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// The base64url alphabet, without padding.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Runs and stops proxy supervisors as detached processes, tracked in the state store.
pub(crate) struct DetachedSupervisor {
    /// Spawns, probes and signals the supervisor process.
    proc: Arc<dyn Proc>,
    /// Time source for leases, waits and sleeps.
    clock: Arc<dyn Clock>,
    /// Source of nonce bytes.
    rng: Arc<dyn Rng>,
    /// The state directory, when one could be resolved.
    state_dir: Option<PathBuf>,
    /// The state store, opened on first use.
    store: OnceLock<Store>,
    /// Restart an unready row under its stored nonce instead of drawing a new one.
    keep_nonce: bool,
}

impl DetachedSupervisor {
    /// A host over `state_dir`; the store opens on first use.
    pub(crate) fn new(
        proc: Arc<dyn Proc>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
        state_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            proc,
            clock,
            rng,
            state_dir,
            store: OnceLock::new(),
            keep_nonce: false,
        }
    }

    /// Make [`ensure`](ProxyHost::ensure) reuse an existing row's nonce when it restarts the
    /// supervisor: a Box started before a controller restart still holds that nonce.
    pub(crate) const fn keeping_nonce(mut self) -> Self {
        self.keep_nonce = true;
        self
    }

    /// The state store, opened on first call.
    fn store(&self) -> Result<&Store, StoreError> {
        if let Some(store) = self.store.get() {
            return Ok(store);
        }
        let dir = self.state_dir.as_deref().ok_or_else(|| {
            StoreError::from(io::Error::new(
                io::ErrorKind::NotFound,
                "no state directory: set MOOR_STATE_DIR, XDG_STATE_HOME or HOME",
            ))
        })?;
        let store = Store::open(&dir.join(STORE_FILE), Arc::clone(&self.clock))?;
        Ok(self.store.get_or_init(|| store))
    }

    /// Whether the row's supervisor holds a live lease now.
    ///
    /// A heartbeat further ahead than [`SKEW`] means the clock stepped back, so the row is stale.
    pub(crate) fn live(&self, row: &ProxyRow) -> bool {
        match (&row.lease_owner, row.lease_heartbeat) {
            (Some(_), Some(beat)) => {
                let age = self
                    .clock
                    .now()
                    .as_millis()
                    .saturating_sub(beat.as_millis());
                let skew = SKEW.as_millis().try_into().unwrap_or(i64::MAX);
                let window = LEASE.as_millis().try_into().unwrap_or(i64::MAX);
                age > -skew && age < window
            }
            _ => false,
        }
    }

    /// Whether the row was written in the current host boot.
    ///
    /// A host without a boot id cannot tell, so every row counts as current there.
    pub(crate) fn same_boot(&self, row: &ProxyRow) -> bool {
        self.proc
            .boot_id()
            .is_none_or(|now| row.boot_id.as_deref() == Some(now.as_str()))
    }

    /// Send SIGTERM to the process group led by `pid`.
    fn signal_group(&self, pid: u32) -> Sent {
        match self.proc.signal_group(pid, Signal::Term) {
            Ok(()) => Sent::Delivered,
            Err(ProcError::Spawn(e)) if e.kind() == io::ErrorKind::NotFound => Sent::Gone,
            Err(_) => Sent::Failed,
        }
    }

    /// Poll until the row is gone; false when it outlasts [`STOP_WAIT`].
    fn wait_gone(&self, store: &Store, task: BoxId) -> bool {
        let start = self.clock.instant();
        loop {
            if !matches!(store.proxy(task), Ok(Some(_))) {
                return true;
            }
            if self.clock.instant().saturating_duration_since(start) >= STOP_WAIT {
                return false;
            }
            self.clock.sleep(POLL);
        }
    }
}

/// What happened to a SIGTERM sent to a process group.
enum Sent {
    /// The signal was delivered.
    Delivered,
    /// No such group: the supervisor is already gone.
    Gone,
    /// The group was refused or could not be signalled.
    Failed,
}

/// Fail the launch at the proxy step.
fn fail(reason: &str, log: &Path) -> ProxyHostError {
    ProxyHostError::Failed(format!("{reason}\n{}", tail(log)))
}

/// Map a store failure to a one-line launch failure.
fn store_failed(e: &StoreError, log: &Path) -> ProxyHostError {
    fail(&format!("the proxy state could not be used: {e}"), log)
}

/// Map a spec or CA file failure to a one-line launch failure.
fn spec_failed(e: &io::Error, log: &Path) -> ProxyHostError {
    fail(&format!("the proxy files could not be written: {e}"), log)
}

/// The endpoint a ready row describes.
fn endpoint(row: &ProxyRow, ca_pem: &str) -> ProxyEndpoint {
    ProxyEndpoint {
        box_port: row.box_port,
        nonce: Secret::new(row.nonce.expose().to_vec()),
        ca_pem: ca_pem.to_owned(),
    }
}

impl ProxyHost for DetachedSupervisor {
    /// Make sure the Task's proxy supervisor is running and ready; return its endpoint.
    ///
    /// A ready supervisor with a live lease is reused. Otherwise a new nonce is reserved and a
    /// supervisor is started, unless one already holds a live lease. The nonce never appears in
    /// arguments, environment, logs or errors.
    fn ensure(&self, spec: &ProxySpec) -> Result<ProxyEndpoint, ProxyHostError> {
        let task = spec.box_id;
        let Some(state) = self.state_dir.as_deref() else {
            return Err(fail(
                "the state directory is not usable: no state directory: set MOOR_STATE_DIR, \
                 XDG_STATE_HOME or HOME",
                Path::new(""),
            ));
        };
        let log = log_path(state, task);
        write_spec(state, spec).map_err(|e| spec_failed(&e, &log))?;
        let ca = load_or_create(state, task).map_err(|e| spec_failed(&e, &log))?;
        let ca_pem = ca.cert_pem();
        let store = self.store().map_err(|e| store_failed(&e, &log))?;
        let row = store.proxy(task).map_err(|e| store_failed(&e, &log))?;
        let kept = row.as_ref().map(|r| Secret::new(r.nonce.expose().to_vec()));
        if let Some(row) = row.filter(|r| r.ready && self.live(r)) {
            return Ok(endpoint(&row, ca_pem));
        }

        let nonce = kept
            .filter(|_| self.keep_nonce)
            .unwrap_or_else(|| new_nonce(self.rng.as_ref()));
        let reserved = store
            .reserve(task, &nonce, self.rng.as_ref(), LEASE)
            .map_err(|e| store_failed(&e, &log))?;
        let pid = match reserved {
            Reserve::Reserved(_) => {
                let spec = ProcSpec::new(Tool::MoorSelf, Deadline(WAIT))
                    .args(["proxy", "serve", &task.get().to_string()])
                    // The controller may run on a state dir its own config chose.
                    .env("MOOR_STATE_DIR", state.to_string_lossy());
                let pid = self.proc.spawn_daemon(&spec, &log).map_err(|e| {
                    fail(&format!("the proxy supervisor could not start: {e}"), &log)
                })?;
                Some(pid)
            }
            Reserve::Leased { .. } => None,
        };

        let start = self.clock.instant();
        loop {
            let row = store.proxy(task).map_err(|e| store_failed(&e, &log))?;
            if let Some(row) = &row {
                let lease = self.live(row);
                if row.ready && lease {
                    return Ok(endpoint(row, ca_pem));
                }
                if !lease && pid.is_some_and(|p| !self.proc.alive(p)) {
                    return Err(fail(
                        "the proxy supervisor exited before it was ready",
                        &log,
                    ));
                }
            }
            if self.clock.instant().saturating_duration_since(start) >= WAIT {
                return Err(fail("the proxy supervisor was not ready after 30s", &log));
            }
            self.clock.sleep(POLL);
        }
    }

    /// Classify the Task's proxy row without starting or changing anything.
    fn status(&self, task: BoxId) -> Result<ProxyStatus, ProxyHostError> {
        let failed = |e: &StoreError| {
            ProxyHostError::Failed(format!("the proxy state could not be used: {e}"))
        };
        let store = self.store().map_err(|e| failed(&e))?;
        let Some(row) = store.proxy(task).map_err(|e| failed(&e))? else {
            return Ok(ProxyStatus::Absent);
        };
        let live = self.live(&row);
        Ok(match (live, row.ready) {
            (true, true) => ProxyStatus::Ready {
                box_port: row.box_port,
            },
            (true, false) => ProxyStatus::Starting,
            (false, _) => ProxyStatus::Stale,
        })
    }

    /// Stop the Task's proxy supervisor, then remove its row and CA directory.
    ///
    /// SIGTERM goes to the supervisor's process group only while its lease is live and the row
    /// was written in the current host boot, because a pid alone may have been reused. The row is
    /// removed whether or not the supervisor exited in time; a supervisor that outlasts the wait
    /// is reported as incomplete and keeps its CA directory. With no row only the CA directory is removed.
    fn stop(&self, task: BoxId) -> Result<(), ProxyHostError> {
        let incomplete = |reason: String| Err(ProxyHostError::Incomplete(reason));
        let store = match self.store() {
            Ok(store) => store,
            Err(e) => return incomplete(format!("the proxy state could not be read: {e}")),
        };
        let row = match store.proxy(task) {
            Ok(Some(row)) => row,
            Ok(None) => {
                // A retried stop finds no row after an earlier one timed out past removing it;
                // with no row there is no lease, so no live supervisor holds the directory.
                if let Some(state) = self.state_dir.as_deref() {
                    if let Err(e) = super::nono::ca::remove(state, task) {
                        return incomplete(format!("the proxy CA could not be removed: {e}"));
                    }
                }
                return Ok(());
            }
            Err(e) => return incomplete(format!("the proxy state could not be read: {e}")),
        };
        // Pids 0 and 1 would address every process or init's group. After a reboot the pid may
        // belong to an unrelated group even though the heartbeat looks fresh.
        let pid = row
            .pid
            .filter(|p| *p > 1 && self.live(&row) && self.same_boot(&row));
        let exited = pid.is_none_or(|p| match self.signal_group(p) {
            Sent::Delivered => self.wait_gone(store, task),
            Sent::Gone => true,
            Sent::Failed => false,
        });
        if let Err(e) = store.remove_proxy(task) {
            return incomplete(format!("the proxy row could not be removed: {e}"));
        }
        if exited {
            if let Some(state) = self.state_dir.as_deref() {
                if let Err(e) = super::nono::ca::remove(state, task) {
                    return incomplete(format!("the proxy CA could not be removed: {e}"));
                }
            }
            Ok(())
        } else {
            incomplete("the proxy supervisor did not exit within 3s".to_owned())
        }
    }
}

/// Where the Task's proxy supervisor writes its log.
pub(crate) fn log_path(state_dir: &Path, task: BoxId) -> PathBuf {
    state_dir.join("proxy").join(format!("{}.log", task.get()))
}

/// A fresh nonce: 32 random bytes as base64url text.
pub(crate) fn new_nonce(rng: &dyn Rng) -> Secret {
    let bytes: Vec<u8> = (0..8).flat_map(|_| rng.u32().to_le_bytes()).collect();
    Secret::new(base64url(&bytes).into_bytes())
}

/// Encode bytes as base64url without padding.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let acc = chunk
            .iter()
            .chain(std::iter::repeat(&0))
            .take(3)
            .fold(0_u32, |a, b| (a << 8) | u32::from(*b));
        for shift in [18_u32, 12, 6, 0].iter().take(chunk.len() + 1) {
            let idx = usize::try_from((acc >> shift) & 63).unwrap_or(0);
            if let Some(c) = ALPHABET.get(idx) {
                out.push(char::from(*c));
            }
        }
    }
    out
}

/// The last lines of the log; empty when it cannot be read.
fn tail(log: &Path) -> String {
    let Ok(text) = fs::read_to_string(log) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines
        .iter()
        .skip(lines.len().saturating_sub(TAIL_LINES))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
