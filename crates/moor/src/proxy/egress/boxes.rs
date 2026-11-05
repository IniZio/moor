//! Each token's Box, and the proxy this replica runs for each Box it serves.
//!
//! A token is looked up on the ledger on every new tunnel, so a Box that closed is refused at
//! once. While the controller cannot be reached, the last answer for the token is used, as a
//! Box's own Proxy uses its last answers.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::seam::clock::Clock;
use moor_core::{BoxId, Secret};
use zeroize::Zeroizing;

use super::Config;
use crate::proxy::ledger::{BoxLedger, EgressLedger, LedgerError};
use crate::proxy::nono::capture::UserSecretsFn;
use crate::proxy::supervisor::{engine, Engine, Parts};

/// Where a tunnel for a Box goes.
pub(super) struct Route {
    /// The Box's nono.
    pub(super) addr: SocketAddr,
    /// The token that nono checks.
    pub(super) internal: Secret,
}

/// Why a token gets no route.
pub(super) enum Refused {
    /// The token is unknown or its Box closed.
    Unknown,
    /// The controller cannot be reached and the token was never seen, or the Box's proxy could
    /// not start.
    Unavailable(String),
}

/// A Box this replica serves.
struct Served {
    /// The Box's store calls, under the egress key.
    ledger: Arc<BoxLedger>,
    /// Its proxy.
    engine: Engine,
}

/// Every Box this replica serves.
pub(super) struct Boxes {
    /// Token lookups.
    lookup: EgressLedger,
    /// The ledger socket each Box's calls go to.
    socket: PathBuf,
    /// The egress key.
    key: Zeroizing<String>,
    /// Where each Box's CA and log go.
    work_dir: PathBuf,
    /// Extra upstream root PEM; `None` in production.
    upstream_ca: Option<PathBuf>,
    /// Loopback addresses the egress guard lets through.
    guard_exempt: Vec<SocketAddr>,
    /// Stamps the request logs.
    clock: Arc<dyn Clock>,
    /// The last Box each token hash was found to belong to.
    known: Mutex<HashMap<String, BoxId>>,
    /// The proxy of each Box served.
    served: Mutex<HashMap<BoxId, Served>>,
}

impl Boxes {
    /// No Box served yet.
    pub(super) fn new(cfg: Config) -> Self {
        Self {
            lookup: EgressLedger::at(cfg.ledger.clone(), cfg.key.to_string()),
            socket: cfg.ledger,
            key: cfg.key,
            work_dir: cfg.work_dir,
            upstream_ca: cfg.upstream_ca,
            guard_exempt: cfg.guard_exempt,
            clock: cfg.clock,
            known: Mutex::default(),
            served: Mutex::default(),
        }
    }

    /// The route for the Box whose token hashes to `token_hash`, starting its proxy on first use.
    pub(super) fn admit(&self, token_hash: &str) -> Result<Route, Refused> {
        let box_id = self.owner(token_hash)?;
        let mut served = lock(&self.served);
        let s = match served.entry(box_id) {
            Entry::Occupied(found) => found.into_mut(),
            Entry::Vacant(free) => free.insert(self.start(box_id).map_err(Refused::Unavailable)?),
        };
        let route = Route {
            addr: s.engine.addr(),
            internal: Secret::new(s.engine.internal().expose().to_vec()),
        };
        drop(served);
        Ok(route)
    }

    /// The open Box of `token_hash`, or the last answer while the controller is away.
    fn owner(&self, token_hash: &str) -> Result<BoxId, Refused> {
        let answer = self.lookup.token_box(token_hash);
        let mut known = lock(&self.known);
        match answer {
            Ok(Some(box_id)) => {
                known.insert(token_hash.to_owned(), box_id);
                Ok(box_id)
            }
            Ok(None) => {
                known.remove(token_hash);
                Err(Refused::Unknown)
            }
            Err(e @ LedgerError::Unavailable(_)) => known
                .get(token_hash)
                .copied()
                .ok_or_else(|| Refused::Unavailable(e.to_string())),
            Err(e) => Err(Refused::Unavailable(e.to_string())),
        }
    }

    /// Starts the proxy of `box_id` from the spec and CA the controller keeps.
    fn start(&self, box_id: BoxId) -> Result<Served, String> {
        let ledger = Arc::new(BoxLedger::egress(
            self.socket.clone(),
            box_id,
            self.key.to_string(),
        ));
        let spec = ledger
            .spec()
            .map_err(|e| format!("cannot read the proxy spec: {e}"))?
            .ok_or_else(|| format!("no proxy spec for Box {}", box_id.get()))?;
        let reads = Arc::clone(&ledger);
        let secrets: UserSecretsFn =
            Arc::new(move |name| reads.secret(name).map_err(|e| e.to_string()));
        let parts = Parts {
            ledger: &ledger,
            secrets: &secrets,
            clock: &self.clock,
            state_dir: &self.work_dir,
            upstream_ca: self.upstream_ca.as_deref(),
            upstream_dial: None,
            guard_exempt: &self.guard_exempt,
        };
        let engine = engine(&parts, box_id, &spec)?;
        Ok(Served { ledger, engine })
    }

    /// Delivers each Box's audit and stops the proxy of each Box that closed.
    pub(super) fn tend(&self) {
        let mut served = lock(&self.served);
        let closed: Vec<BoxId> = served
            .iter()
            .filter(|(_, s)| {
                s.engine.drain(&s.ledger);
                s.ledger.flush();
                match s.ledger.view() {
                    Ok(view) => !view.is_some_and(|v| v.open),
                    Err(LedgerError::Unavailable(_)) => false,
                    Err(_) => true,
                }
            })
            .map(|(id, _)| *id)
            .collect();
        let gone: Vec<Served> = closed.iter().filter_map(|id| served.remove(id)).collect();
        drop(served);
        lock(&self.known).retain(|_, id| !closed.contains(id));
        for s in gone {
            s.engine.stop();
        }
    }

    /// Delivers what audit is left and stops every proxy.
    pub(super) fn stop(&self) {
        let all: Vec<Served> = lock(&self.served).drain().map(|(_, s)| s).collect();
        for s in all {
            s.engine.drain(&s.ledger);
            s.ledger.flush();
            s.engine.stop();
        }
    }
}

/// `m`, locked, whatever a panicking holder left.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
