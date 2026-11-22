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
//! request for a host the Proxy serves itself, the machine service or moor's own, goes to its
//! upstream instead, carrying the Box, and for the machine service the machine it holds, in
//! `X-Moor-*` headers that only the Proxy sets. An upstream that does not answer, or whose name
//! reaches an address that is neither loopback nor private, is `502` before the upgrade, and a
//! CONNECT to a served host is `403`. A request that is not one plain request for
//! the target gets a `403` reply in the tunnel.
//!
//! A remote front's turn takes the same path: its forwarder sends the lease as
//! `Authorization: Basic`, and the tunnel goes through a proxy holding that front's rules for the
//! lease's user. Only a token shaped as a lease takes this path, and it never reaches the machine
//! service.
//!
//! `GET /ready` answers `200` while the Proxy takes new tunnels, with a body naming its key
//! (`token::key_id`), so a controller can tell a Proxy under another key apart. On SIGTERM it stops listening,
//! so the probe fails and new tunnels go to another replica, and open tunnels get up to the
//! grace to finish.

mod boxes;
mod limit;
mod listen;
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

use moor_core::config::{Served, Upstream};
use moor_core::seam::clock::Clock;

use tokio::sync::Notify;
use zeroize::Zeroizing;

use super::ledger::Endpoint;
use super::Sink;
use boxes::warm::keep_warm;
pub(crate) use boxes::warm::Warm;
use boxes::Boxes;
pub(crate) use limit::Rate;
pub(crate) use listen::Listen;
use listen::Listener;

/// The path the forwarder opens its WebSocket on.
pub(crate) const PATH: &str = "/egress";
/// Answers 200 from the first whole warming pass on; rollouts wait on it.
pub(crate) const SYNCED: &str = "/synced";

/// A host the Proxy serves itself, as `--machine-service HOST=ADDR` or `--moor-service HOST=ADDR`
/// names it: a Box's plain requests for `host` go to `upstream`, `host:port` with its name looked
/// up on each connection and dialled only when every address is loopback or private.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ServedRoute {
    /// What it is for, which decides the headers stamped on each request.
    pub(crate) served: Served,
    /// Name a Box requests over `http://`, compared without case.
    pub(crate) host: String,
    /// Where those requests go.
    pub(crate) upstream: String,
    /// Sent in `X-Moor-Service-Key` on each request, so the service trusts the Box headers.
    pub(crate) key: Option<Arc<str>>,
}

impl std::fmt::Debug for ServedRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.host, self.upstream)
    }
}

impl ServedRoute {
    /// `HOST=ADDR` for `served`; none when either side is empty or `ADDR` is not an upstream.
    pub(crate) fn parse(served: Served, value: &str) -> Option<Self> {
        let (host, upstream) = value.split_once('=')?;
        let upstream = Upstream::parse(upstream).ok()?;
        (!host.is_empty()).then(|| Self {
            served,
            host: host.to_owned(),
            upstream: upstream.as_str().to_owned(),
            key: None,
        })
    }

    /// True when `host` (no port) names this service.
    fn serves(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(&self.host)
    }
}

/// The header naming the CONNECT target as `host:port`.
pub(crate) const TARGET: &str = "x-moor-target";
/// The header that marks a tunnel carrying one plain `http://` request.
pub(crate) const PLAIN: &str = "x-moor-plain";

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
    /// The hosts it serves itself, such as the machine service.
    pub(crate) served: Vec<ServedRoute>,
    /// How the replica keeps every awake Box ready; `None` learns each Box on first use only,
    /// as one Proxy beside its controller does.
    pub(crate) warm: Option<Warm>,
    /// Source of time.
    pub(crate) clock: Arc<dyn Clock>,
}

/// What every connection shares.
struct Shared {
    /// Each token's Box and its proxy.
    boxes: Boxes,
    /// Connections accepted and not yet finished.
    open: AtomicUsize,
    /// The hosts it serves itself.
    served: Vec<ServedRoute>,
    /// Names the egress key on `/ready`.
    key_id: i64,
}

/// A running Proxy.
pub(crate) struct Server {
    /// Runs the listener and every tunnel.
    runtime: tokio::runtime::Runtime,
    /// Where it listens.
    listening: Listen,
    /// Each accept loop, which holds its listener until it returns, and what wakes it to stop.
    accepting: Vec<(Arc<Notify>, tokio::task::JoinHandle<()>)>,
    /// Shared with connections.
    shared: Arc<Shared>,
    /// Set to stop the tending thread.
    tending: Arc<AtomicBool>,
    /// The tending thread.
    tender: Option<std::thread::JoinHandle<()>>,
    /// The warming thread, when the replica warms.
    warmer: Option<std::thread::JoinHandle<()>>,
}

/// Starts the thread that tends `shared`'s Boxes every `every` while `on` is set.
fn spawn_tender(
    on: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    clock: Arc<dyn Clock>,
    every: Duration,
) -> Result<std::thread::JoinHandle<()>, String> {
    let (on, shared) = (Arc::clone(on), Arc::clone(shared));
    std::thread::Builder::new()
        .name("moor-egress-tend".to_owned())
        .spawn(move || {
            while on.load(Ordering::Acquire) {
                clock.sleep(every);
                shared.boxes.tend();
            }
        })
        .map_err(|e| format!("cannot start tending: {e}"))
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
        let (served, warm) = (cfg.served.clone(), cfg.warm);
        let key_id = crate::proxy::token::key_id(cfg.key.as_bytes());
        let shared = Arc::new(Shared {
            boxes: Boxes::new(cfg),
            open: AtomicUsize::new(0),
            served,
            key_id,
        });
        let stop = Arc::new(Notify::new());
        let serving = runtime.spawn(accept(listener, Arc::clone(&shared), Arc::clone(&stop)));
        let accepting = vec![(stop, serving)];
        let tending = Arc::new(AtomicBool::new(true));
        let thread = spawn_tender(&tending, &shared, Arc::clone(&clock), every)?;
        let warmer = warm
            .map(|warm| {
                let (on, shared) = (Arc::clone(&tending), Arc::clone(&shared));
                std::thread::Builder::new()
                    .name("moor-egress-warm".to_owned())
                    .spawn(move || keep_warm(&shared.boxes, warm, (&on, clock.as_ref(), every)))
            })
            .transpose()
            .map_err(|e| format!("cannot start warming: {e}"))?;
        Ok(Self {
            runtime,
            listening,
            accepting,
            shared,
            tending,
            tender: Some(thread),
            warmer,
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

    /// Where each running Box engine takes its tunnels.
    #[cfg(test)]
    pub(crate) fn engines(&self) -> Vec<SocketAddr> {
        self.shared.boxes.engines()
    }

    /// How many Boxes have a running proxy.
    #[cfg(test)]
    pub(crate) fn serving(&self) -> usize {
        self.shared.boxes.serving()
    }

    /// Stops listening, waits up to `grace` for open tunnels to finish while tending goes on,
    /// then stops the rest; returns how many were still open.
    pub(crate) fn drain(mut self, grace: Duration) -> usize {
        let (shared, accepting) = (
            Arc::clone(&self.shared),
            std::mem::take(&mut self.accepting),
        );
        for (stop, _) in &accepting {
            stop.notify_one();
        }
        let left = self.runtime.block_on(async move {
            for (_, serving) in accepting {
                drop(serving.await);
            }
            let waited = tokio::time::timeout(grace, async {
                while shared.open.load(Ordering::Acquire) > 0 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
            drop(waited.await);
            shared.open.load(Ordering::Acquire)
        });
        self.tending.store(false, Ordering::Release);
        for thread in [self.tender.take(), self.warmer.take()]
            .into_iter()
            .flatten()
        {
            drop(thread.join());
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
