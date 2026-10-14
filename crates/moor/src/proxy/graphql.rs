//! Loopback filter that nono chains GitHub API traffic through so each
//! GraphQL request is judged against the Task repo before it leaves.

use std::collections::VecDeque;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use moor_core::graphql_scope::RepoRef;
use nono_proxy::tls_intercept::{build_server_config, CertCache, EphemeralCa};
use rustls::ServerConfig;

use crate::proxy::guard::Guard;
use lookup::NodeCache;
use upstream::Upstream;

mod gate;
pub(crate) mod http;
pub(crate) mod lookup;
mod serve;
mod tunnel;
pub(crate) mod upstream;

#[cfg(test)]
mod tests;

/// Category reported for a request the filter refused.
pub(crate) const SCOPE_CATEGORY: &str = "graphql_scope";

/// Category reported for a tunnel the destination guard refused.
pub(crate) const GUARD_CATEGORY: &str = "destination_guard";

/// Rule name reported with destination guard refusals.
const EGRESS_RULE: &str = "egress";

/// Most refusals kept before the oldest is dropped.
const MAX_REFUSALS: usize = 1024;

/// What the filter needs for one Task.
pub(crate) struct FilterSpec {
    /// The Task CA the filter presents to nono.
    pub(crate) ca: EphemeralCa,
    /// The Task repo, if known; without it every GraphQL request is refused.
    pub(crate) repo: Option<RepoRef>,
    /// The only host the filter tunnels to, such as `api.github.com`.
    pub(crate) host: String,
    /// The port of that host, such as 443.
    pub(crate) port: u16,
    /// Name of the proxy rule, reported with each refusal.
    pub(crate) rule: String,
    /// Connections to the real upstream.
    pub(crate) upstream: Upstream,
    /// Vets every other tunnel destination.
    pub(crate) guard: Guard,
}

/// One refused request, reported without body, path, query or headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The host the request was for.
    pub(crate) host: String,
    /// The proxy rule that chained the request here.
    pub(crate) rule: String,
    /// The request method.
    pub(crate) method: String,
    /// Why it was refused, as a stable category.
    pub(crate) category: &'static str,
}

/// Why the filter could not start.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FilterError {
    /// The loopback listener could not be set up.
    #[error("graphql filter listener failed: {0}")]
    Listen(#[source] std::io::Error),
    /// The TLS server settings could not be built.
    #[error("graphql filter TLS setup failed: {0}")]
    Tls(String),
}

/// State shared by the acceptor and every connection thread.
pub(super) struct Shared {
    /// The Task repo, if known.
    pub(super) repo: Option<RepoRef>,
    /// The only tunnel host.
    pub(super) host: String,
    /// The only tunnel port.
    pub(super) port: u16,
    /// Rule name for refusals.
    pub(super) rule: String,
    /// The real upstream.
    pub(super) upstream: Upstream,
    /// Vets destinations other than the judged host.
    pub(super) guard: Guard,
    /// TLS settings for client connections.
    pub(super) tls: Arc<ServerConfig>,
    /// Node-id verdicts seen so far.
    pub(super) cache: NodeCache,
    /// Refusals not yet drained.
    refusals: Mutex<VecDeque<Refusal>>,
    /// Set when the filter is stopping.
    stop: AtomicBool,
}

impl Shared {
    /// Records a refusal for the judged host.
    pub(super) fn refuse(&self, method: &str, category: &'static str) {
        self.refuse_host(&self.host, &self.rule, method, category);
    }

    /// Records a refusal, dropping the oldest when the buffer is full.
    pub(super) fn refuse_host(&self, host: &str, rule: &str, method: &str, category: &'static str) {
        let mut q = self.lock_refusals();
        if q.len() >= MAX_REFUSALS {
            q.pop_front();
        }
        q.push_back(Refusal {
            host: host.to_owned(),
            rule: rule.to_owned(),
            method: method.to_owned(),
            category,
        });
    }

    /// Locks the refusal buffer, recovering from poisoning.
    fn lock_refusals(&self) -> MutexGuard<'_, VecDeque<Refusal>> {
        self.refusals.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A running filter on a loopback port.
pub(crate) struct GraphqlFilter {
    /// State shared with the threads.
    shared: Arc<Shared>,
    /// Where it listens.
    addr: SocketAddr,
    /// The acceptor thread.
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for GraphqlFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphqlFilter")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl GraphqlFilter {
    /// Binds `127.0.0.1:0` and starts accepting.
    pub(crate) fn start(spec: FilterSpec) -> Result<Self, FilterError> {
        let tls = build_server_config(Arc::new(CertCache::new(Arc::new(spec.ca))), false)
            .map_err(|e| FilterError::Tls(e.to_string()))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(FilterError::Listen)?;
        let addr = listener.local_addr().map_err(FilterError::Listen)?;
        let shared = Arc::new(Shared {
            repo: spec.repo,
            host: spec.host,
            port: spec.port,
            rule: spec.rule,
            upstream: spec.upstream,
            guard: spec.guard,
            tls,
            cache: NodeCache::new(),
            refusals: Mutex::new(VecDeque::new()),
            stop: AtomicBool::new(false),
        });
        let acceptor = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("graphql-filter".to_owned())
            .spawn(move || accept_loop(&listener, &acceptor))
            .map_err(FilterError::Listen)?;
        Ok(Self {
            shared,
            addr,
            thread: Some(thread),
        })
    }

    /// The loopback address nono chains through.
    pub(crate) const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Takes the refusals recorded since the last call.
    pub(crate) fn drain_refusals(&self) -> Vec<Refusal> {
        self.shared.lock_refusals().drain(..).collect()
    }

    /// Stops accepting and joins the acceptor; open connections end on their
    /// socket timeouts.
    pub(crate) fn stop(mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        drop(TcpStream::connect(self.addr));
        if let Some(t) = self.thread.take() {
            drop(t.join());
        }
    }
}

/// Accepts connections until stopped, one thread each.
fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for conn in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(tcp) = conn else { continue };
        let shared = Arc::clone(shared);
        drop(
            std::thread::Builder::new()
                .name("graphql-conn".to_owned())
                .spawn(move || serve::connection(tcp, &shared)),
        );
    }
}
