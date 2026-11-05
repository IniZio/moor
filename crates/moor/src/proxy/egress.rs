//! The shared egress Proxy: one process that serves the credential traffic of every Box, keyed
//! by each Box's token, run by `moor proxy serve-egress`. It is off unless started, and the
//! controller answers its ledger hello only when both share `MOOR_EGRESS_KEY`.
//!
//! # Wire contract
//!
//! The in-Box forwarder opens one WebSocket per CONNECT it accepts:
//!
//! ```text
//! GET /egress HTTP/1.1
//! Host: <proxy host>
//! Connection: Upgrade
//! Upgrade: websocket
//! Sec-WebSocket-Version: 13
//! Sec-WebSocket-Key: <key>
//! Authorization: Bearer <box token>
//! X-Moor-Target: <host>:<port>
//! ```
//!
//! The Proxy opens the CONNECT to the target before it answers, so the answer is the CONNECT's:
//!
//! - `101 Switching Protocols`: the tunnel is open. The forwarder answers its client
//!   `200 Connection established` and copies bytes both ways.
//! - `400`: not a WebSocket upgrade, or `X-Moor-Target` is missing or not `host:port`.
//! - `401`: the token is missing or unknown, or its Box has closed.
//! - `404`: a path other than `/egress` or `/ready`.
//! - `429`: the token opened more tunnels than its rate allows, or too many unknown tokens were
//!   tried. Retry after a second.
//! - `503`: the controller cannot be reached and the token was never seen, or the Box's proxy
//!   could not start.
//! - Any other status is the Box's proxy refusing the CONNECT, `403` for a host it refuses.
//!   The forwarder passes it to its client.
//!
//! After `101`, each binary message carries tunnel bytes in order. A close frame or the end of
//! the connection ends the tunnel. Any other data message ends it too.
//!
//! `GET /ready` answers `200` while the Proxy takes new tunnels. On SIGTERM it stops listening,
//! so the probe fails and new tunnels go to another replica, and open tunnels get up to the
//! grace to finish.

mod boxes;
mod limit;
pub(crate) mod run;
#[cfg(test)]
mod tests;
mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Clock;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use zeroize::Zeroizing;

use boxes::Boxes;
pub(crate) use limit::Rate;

/// The path the forwarder opens its WebSocket on.
pub(crate) const PATH: &str = "/egress";
/// The header naming the CONNECT target as `host:port`.
pub(crate) const TARGET: &str = "x-moor-target";

/// How the Proxy runs.
pub(crate) struct Config {
    /// Where it listens.
    pub(crate) listen: SocketAddr,
    /// The controller's ledger socket.
    pub(crate) ledger: PathBuf,
    /// The key it presents on the ledger.
    pub(crate) key: Zeroizing<String>,
    /// Where each Box's CA and request log go.
    pub(crate) work_dir: PathBuf,
    /// The tunnel rate of each token the ledger confirmed.
    pub(crate) rate: Rate,
    /// The lookup rate shared by tokens not yet confirmed.
    pub(crate) lookups: Rate,
    /// Extra upstream root PEM the Box proxies trust; `None` in production.
    pub(crate) upstream_ca: Option<PathBuf>,
    /// Loopback addresses the egress guard lets through; empty in production.
    pub(crate) guard_exempt: Vec<SocketAddr>,
    /// Source of time.
    pub(crate) clock: Arc<dyn Clock>,
}

/// What every connection shares.
struct Shared {
    /// Each token's Box and its proxy.
    boxes: Boxes,
    /// Connections accepted and not yet finished.
    open: AtomicUsize,
}

/// A running Proxy.
pub(crate) struct Server {
    /// Runs the listener and every tunnel.
    runtime: tokio::runtime::Runtime,
    /// Where it listens.
    addr: SocketAddr,
    /// Wakes the accept loop to stop listening.
    stop: Arc<Notify>,
    /// Shared with connections.
    shared: Arc<Shared>,
}

impl Server {
    /// Binds `cfg.listen` and starts serving.
    pub(crate) fn start(cfg: Config) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("moor-egress")
            .build()
            .map_err(|e| format!("cannot build the runtime: {e}"))?;
        let listener = runtime
            .block_on(TcpListener::bind(cfg.listen))
            .map_err(|e| format!("cannot listen on {}: {e}", cfg.listen))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("cannot read the listen address: {e}"))?;
        let shared = Arc::new(Shared {
            boxes: Boxes::new(cfg),
            open: AtomicUsize::new(0),
        });
        let stop = Arc::new(Notify::new());
        runtime.spawn(accept(listener, Arc::clone(&shared), Arc::clone(&stop)));
        Ok(Self {
            runtime,
            addr,
            stop,
            shared,
        })
    }

    /// Where it listens.
    pub(crate) const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// How many tokens this replica keeps.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.shared.boxes.tracked()
    }

    /// Delivers each Box's audit and drops the proxies of Boxes that closed.
    pub(crate) fn tend(&self) {
        self.shared.boxes.tend();
    }

    /// Stops listening, waits up to `grace` for open tunnels to finish, then stops the rest;
    /// returns how many were still open.
    pub(crate) fn drain(self, grace: Duration) -> usize {
        self.stop.notify_one();
        let shared = Arc::clone(&self.shared);
        let left = self.runtime.block_on(async move {
            let waited = tokio::time::timeout(grace, async {
                while shared.open.load(Ordering::Acquire) > 0 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
            drop(waited.await);
            shared.open.load(Ordering::Acquire)
        });
        self.runtime.shutdown_background();
        self.shared.boxes.stop();
        left
    }
}

/// Accepts connections until `stop`, then drops the listener so new ones are refused.
async fn accept(listener: TcpListener, shared: Arc<Shared>, stop: Arc<Notify>) {
    loop {
        tokio::select! {
            () = stop.notified() => return,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let counted = Counted::new(Arc::clone(&shared));
                tokio::spawn(async move { wire::serve(stream, &counted.0).await });
            }
        }
    }
}

/// One open connection, counted until it is dropped, even when its handler panics.
struct Counted(Arc<Shared>);

impl Counted {
    /// Counts a new connection.
    fn new(shared: Arc<Shared>) -> Self {
        shared.open.fetch_add(1, Ordering::AcqRel);
        Self(shared)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}
