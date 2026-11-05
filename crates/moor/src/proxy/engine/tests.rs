//! The per-Box engine end to end on loopback: requests go through nono and the GraphQL filter
//! to fake HTTPS upstreams, as the shared egress Proxy sends each tunnel into its engine.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::UserId;
use moor_core::{BoxId, BoxName};
use ureq::Agent;

use super::{engine, Engine, Parts};
use crate::proxy::ledger::tests::OnLedger;
use crate::proxy::nono::ca;
use crate::proxy::nono::capture::UserSecretsFn;
use crate::proxy::nono::tests::upstream;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;

mod also_repos;
mod graphql_e2e;
mod hairpin_e2e;
mod nono_e2e;
mod users_e2e;

/// Longest a test waits for something that happens on another thread.
const DEADLINE: Duration = Duration::from_secs(20);

/// The state directory and store the engines of one test share.
pub(super) struct Host {
    /// Keeps the state directory alive.
    pub(super) dir: tempfile::TempDir,
    /// Box and event store.
    pub(super) store: Store,
    /// Clock shared with the store and the engines.
    pub(super) clock: Arc<dyn Clock>,
}

impl Host {
    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let store = Store::open(&dir.path().join(STORE_FILE), Arc::clone(&clock)).unwrap();
        Self { dir, store, clock }
    }

    /// A Box named `name` owned by `user`.
    pub(super) fn new_box(&self, name: &str, user: Option<UserId>) -> BoxId {
        let new = NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: None,
        };
        let decision = user
            .iter()
            .fold(Decision::insert(new), |d, u| d.requester(*u));
        self.store.apply(decision).unwrap().box_id
    }
}

/// What differs between tests in how an engine reaches its upstreams.
pub(super) struct Wiring {
    /// The Box user's secrets.
    pub(super) secrets: UserSecretsFn,
    /// Extra upstream root PEM the proxy trusts.
    pub(super) upstream_ca: Option<PathBuf>,
    /// Address the GraphQL filter dials instead of resolving the GitHub API host.
    pub(super) upstream_dial: Option<SocketAddr>,
    /// Loopback addresses the egress guard lets through.
    pub(super) guard_exempt: Vec<SocketAddr>,
}

/// A started engine with the ledger it reads from and the CA its clients trust.
pub(super) struct Running {
    /// The Box.
    pub(super) box_id: BoxId,
    /// The Box's store calls, as the egress Proxy makes them.
    pub(super) ledger: OnLedger,
    /// The engine; `None` once stopped.
    pub(super) engine: Option<Engine>,
    /// Box CA certificate clients trust.
    pub(super) ca_pem: String,
    /// The store the ledger is served from.
    pub(super) store: Store,
}

impl Running {
    /// Records `spec` for the Box, as the controller does, and starts its engine.
    pub(super) fn start(
        host: &Host,
        box_id: BoxId,
        ledger: OnLedger,
        spec: &ProxySpec,
        wiring: &Wiring,
    ) -> Self {
        host.store.set_proxy_spec(box_id, spec).unwrap();
        let parts = Parts {
            ledger: &ledger,
            secrets: &wiring.secrets,
            clock: &host.clock,
            state_dir: host.dir.path(),
            upstream_ca: wiring.upstream_ca.as_deref(),
            upstream_dial: wiring.upstream_dial,
            guard_exempt: &wiring.guard_exempt,
        };
        let engine = engine(&parts, box_id, spec).unwrap();
        let ca_pem = std::fs::read_to_string(ca::cert_path(host.dir.path(), box_id)).unwrap();
        Self {
            box_id,
            ledger,
            engine: Some(engine),
            ca_pem,
            store: host.store.clone(),
        }
    }

    pub(super) fn engine(&self) -> &Engine {
        self.engine.as_ref().unwrap()
    }

    /// Port of the nono proxy.
    pub(super) fn port(&self) -> u16 {
        self.engine().addr().port()
    }

    /// The token the engine checks.
    pub(super) fn token(&self) -> String {
        String::from_utf8(self.engine().internal().expose().to_vec()).unwrap()
    }

    /// A client that goes through the engine as the egress Proxy does.
    pub(super) fn client(&self) -> Agent {
        upstream::client(self.port(), &self.token(), &self.ca_pem)
    }

    /// Moves recorded audit events into the log and the store.
    pub(super) fn drain(&self) {
        self.engine().drain(&self.ledger);
    }

    /// Kinds and payloads of the Box events recorded so far.
    pub(super) fn events(&self) -> Vec<(String, String)> {
        let rows = self.store.events(self.box_id, 100).unwrap();
        rows.into_iter()
            .map(|e| (e.kind, e.payload.to_string()))
            .collect()
    }

    /// Drains until `done` holds for the events, then once more for the audit nono writes
    /// after the client already has its answer.
    pub(super) fn events_until(
        &self,
        what: &str,
        done: impl Fn(&[(String, String)]) -> bool,
    ) -> Vec<(String, String)> {
        let step = Duration::from_millis(50);
        let tries = DEADLINE.as_millis() / step.as_millis();
        for _ in 0..tries {
            self.drain();
            if done(&self.events()) {
                self.drain();
                return self.events();
            }
            std::thread::sleep(step);
        }
        unreachable!("no {what} in {:?}", self.events())
    }

    /// Stops the engine.
    pub(super) fn stop(mut self) {
        self.engine.take().unwrap().stop();
    }
}
