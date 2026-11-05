//! The proxy this replica runs for one Box, and a tunnel's hold on it.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::seam::proxy_host::ProxySpec;
use moor_core::Secret;

use crate::proxy::ledger::BoxLedger;
use crate::proxy::supervisor::Engine;

/// A Box's proxy, built from one spec. Tunnels hold it, so a proxy replaced after a spec change
/// keeps serving its open tunnels and stops once the last one ends.
pub(super) struct Served {
    /// The Box's store calls, under the egress key.
    pub(super) ledger: Arc<BoxLedger>,
    /// The spec it was built from.
    pub(super) spec: ProxySpec,
    /// Where tunnels go.
    pub(super) addr: SocketAddr,
    /// The token its nono checks.
    pub(super) internal: Secret,
    /// The proxy until it stops.
    engine: Mutex<Option<Engine>>,
}

impl Served {
    /// Wraps a started `engine`.
    pub(super) fn new(ledger: Arc<BoxLedger>, spec: ProxySpec, engine: Engine) -> Self {
        Self {
            ledger,
            spec,
            addr: engine.addr(),
            internal: Secret::new(engine.internal().expose().to_vec()),
            engine: Mutex::new(Some(engine)),
        }
    }

    /// Delivers the audit recorded since the last call.
    pub(super) fn drain(&self) {
        if let Some(engine) = lock(&self.engine).as_ref() {
            engine.drain(&self.ledger);
        }
        self.ledger.flush();
    }

    /// Delivers what audit is left and stops the proxy, cutting its tunnels.
    pub(super) fn stop(&self) {
        let engine = lock(&self.engine).take();
        if let Some(engine) = engine {
            engine.drain(&self.ledger);
            self.ledger.flush();
            engine.stop();
        }
    }

    /// Whether the proxy still runs.
    #[cfg(test)]
    pub(super) fn running(&self) -> bool {
        lock(&self.engine).is_some()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One open tunnel of a token, counted until it is dropped.
pub(super) struct Tunnel(Arc<AtomicUsize>);

impl Tunnel {
    /// Counts a tunnel on `open` unless `cap` are open already.
    pub(super) fn claim(open: &Arc<AtomicUsize>, cap: usize) -> Option<Self> {
        open.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < cap).then_some(n + 1)
        })
        .ok()
        .map(|_| Self(Arc::clone(open)))
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// `m`, locked, whatever a panicking holder left.
pub(super) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
