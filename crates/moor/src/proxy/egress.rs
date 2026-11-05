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
//! With `X-Moor-Plain: 1` the tunnel carries one absolute-form `http://` request for the target,
//! sent whole after `101`, and its reply. The Proxy sends it on in origin form, without
//! `Proxy-*` or hop-by-hop headers, through the Box's proxy and never with a credential. A
//! request for the machine service goes to the service instead, carrying the Box and the machine
//! it holds in `X-Moor-*` headers that only the Proxy sets; the service not answering is `502`
//! before the upgrade, and a CONNECT to it is `403`. A request that is not one plain request for
//! the target gets a `403` reply in the tunnel.
//!
//! `GET /ready` answers `200` while the Proxy takes new tunnels, with a body naming its key
//! (`token::key_id`), so a controller can tell a Proxy under another key apart. On SIGTERM it stops listening,
//! so the probe fails and new tunnels go to another replica, and open tunnels get up to the
//! grace to finish.

mod boxes;
mod limit;
pub(crate) mod run;
mod served;
#[cfg(test)]
mod tests;
mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::MachineService;
use moor_core::seam::clock::Clock;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

use tokio::net::{TcpListener, UnixListener};
use tokio::sync::Notify;
use zeroize::Zeroizing;

use super::ledger::Endpoint;
use super::Sink;
use boxes::Boxes;
pub(crate) use limit::Rate;

/// The path the forwarder opens its WebSocket on.
pub(crate) const PATH: &str = "/egress";
/// The header naming the CONNECT target as `host:port`.
pub(crate) const TARGET: &str = "x-moor-target";
/// The header that marks a tunnel carrying one plain `http://` request.
pub(crate) const PLAIN: &str = "x-moor-plain";

/// Where the Proxy listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Listen {
    /// A TCP address.
    Tcp(SocketAddr),
    /// A Unix socket only moor's user can open, which each Box's tunnel reaches on a dev host.
    Unix(PathBuf),
}

impl std::fmt::Display for Listen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "{addr}"),
            Self::Unix(path) => write!(f, "{}", path.display()),
        }
    }
}

/// A bound listener.
enum Listener {
    /// On TCP.
    Tcp(TcpListener),
    /// On a Unix socket.
    Unix(UnixListener),
}

impl Listen {
    /// Binds it, replacing a stale socket file but never one a live Proxy listens on; what it is bound to, with the port a TCP
    /// address of port 0 got.
    async fn bind(&self) -> std::io::Result<(Listener, Self)> {
        match self {
            Self::Tcp(addr) => {
                let l = TcpListener::bind(addr).await?;
                let bound = l.local_addr()?;
                Ok((Listener::Tcp(l), Self::Tcp(bound)))
            }
            Self::Unix(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(dir)?;
                }
                if std::os::unix::net::UnixStream::connect(path).is_ok() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        "another Proxy listens on it",
                    ));
                }
                match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
                let l = UnixListener::bind(path)?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                Ok((Listener::Unix(l), self.clone()))
            }
        }
    }
}

/// How the Proxy runs.
pub(crate) struct Config {
    /// Where it listens.
    pub(crate) listen: Listen,
    /// Where the controller's ledger is.
    pub(crate) ledger: Endpoint,
    /// The key it presents on the ledger.
    pub(crate) key: Zeroizing<String>,
    /// Where each Box's CA and request log go.
    pub(crate) work_dir: PathBuf,
    /// The tunnel rate of each token the ledger confirmed.
    pub(crate) rate: Rate,
    /// The lookup rate shared by tokens not yet confirmed.
    pub(crate) lookups: Rate,
    /// Most tunnels one token may hold open at once.
    pub(crate) tunnels: usize,
    /// Pause between audit deliveries, closed-Box sweeps and spec checks.
    pub(crate) tend: Duration,
    /// Where failures are logged; no failure detail ever reaches a client.
    pub(crate) log: Sink,
    /// Extra upstream root PEM the Box proxies trust; `None` in production.
    pub(crate) upstream_ca: Option<PathBuf>,
    /// Loopback addresses the egress guard lets through; empty in production.
    pub(crate) guard_exempt: Vec<SocketAddr>,
    /// The machine service plain requests for its host go to; `None` when none is configured.
    pub(crate) machine: Option<MachineService>,
    /// Source of time.
    pub(crate) clock: Arc<dyn Clock>,
}

/// What every connection shares.
struct Shared {
    /// Each token's Box and its proxy.
    boxes: Boxes,
    /// Connections accepted and not yet finished.
    open: AtomicUsize,
    /// The machine service, when one is configured.
    machine: Option<MachineService>,
    /// Names the egress key on `/ready`.
    key_id: i64,
}

/// A running Proxy.
pub(crate) struct Server {
    /// Runs the listener and every tunnel.
    runtime: tokio::runtime::Runtime,
    /// Where it listens.
    listening: Listen,
    /// Wakes the accept loop to stop listening.
    stop: Arc<Notify>,
    /// The accept loop, which holds the listener until it returns.
    accepting: tokio::task::JoinHandle<()>,
    /// Shared with connections.
    shared: Arc<Shared>,
    /// Set to stop the tending thread.
    tending: Arc<AtomicBool>,
    /// The tending thread.
    tender: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Binds `cfg.listen` and starts serving.
    pub(crate) fn start(cfg: Config) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("moor-egress")
            .build()
            .map_err(|e| format!("cannot build the runtime: {e}"))?;
        let (listener, listening) = runtime
            .block_on(cfg.listen.bind())
            .map_err(|e| format!("cannot listen on {}: {e}", cfg.listen))?;
        let (clock, every) = (Arc::clone(&cfg.clock), cfg.tend);
        let machine = cfg.machine.clone();
        let key_id = crate::proxy::token::key_id(cfg.key.as_bytes());
        let shared = Arc::new(Shared {
            boxes: Boxes::new(cfg),
            open: AtomicUsize::new(0),
            machine,
            key_id,
        });
        let stop = Arc::new(Notify::new());
        let accepting = runtime.spawn(accept(listener, Arc::clone(&shared), Arc::clone(&stop)));
        let tending = Arc::new(AtomicBool::new(true));
        let (on, boxes) = (Arc::clone(&tending), Arc::clone(&shared));
        let thread = std::thread::Builder::new()
            .name("moor-egress-tend".to_owned())
            .spawn(move || {
                while on.load(Ordering::Acquire) {
                    clock.sleep(every);
                    boxes.boxes.tend();
                }
            })
            .map_err(|e| format!("cannot start tending: {e}"))?;
        Ok(Self {
            runtime,
            listening,
            stop,
            accepting,
            shared,
            tending,
            tender: Some(thread),
        })
    }

    /// Where it listens.
    pub(crate) const fn listening(&self) -> &Listen {
        &self.listening
    }

    /// How many tokens this replica keeps.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.shared.boxes.tracked()
    }

    /// How many Boxes have a running proxy.
    #[cfg(test)]
    pub(crate) fn serving(&self) -> usize {
        self.shared.boxes.serving()
    }

    /// Stops listening, waits up to `grace` for open tunnels to finish while tending goes on,
    /// then stops the rest; returns how many were still open.
    pub(crate) fn drain(mut self, grace: Duration) -> usize {
        self.stop.notify_one();
        let (shared, accepting) = (Arc::clone(&self.shared), self.accepting);
        let left = self.runtime.block_on(async move {
            drop(accepting.await);
            let waited = tokio::time::timeout(grace, async {
                while shared.open.load(Ordering::Acquire) > 0 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
            drop(waited.await);
            shared.open.load(Ordering::Acquire)
        });
        self.tending.store(false, Ordering::Release);
        if let Some(tender) = self.tender.take() {
            drop(tender.join());
        }
        self.runtime.shutdown_background();
        self.shared.boxes.stop();
        left
    }
}

/// Accepts connections until `stop`, then drops the listener so new ones are refused.
async fn accept(listener: Listener, shared: Arc<Shared>, stop: Arc<Notify>) {
    loop {
        tokio::select! {
            () = stop.notified() => return,
            () = accept_one(&listener, &shared) => {}
        }
    }
}

/// Accepts one connection and serves it while the loop accepts the next.
async fn accept_one(listener: &Listener, shared: &Arc<Shared>) {
    let counted = || Counted::new(Arc::clone(shared));
    match listener {
        Listener::Tcp(l) => {
            if let Ok((stream, _)) = l.accept().await {
                let counted = counted();
                tokio::spawn(async move { wire::serve(stream, &counted.0).await });
            }
        }
        Listener::Unix(l) => {
            if let Ok((stream, _)) = l.accept().await {
                let counted = counted();
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
