//! One nono per front and user, started on the first request that user's lease sends and kept
//! until the Proxy stops.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::os::unix::fs::DirBuilderExt as _;
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::config::proxy::ResolvedRule;
use moor_core::user::UserId;
use moor_core::Secret;

use crate::proxy::ledger::FrontsLedger;
use crate::proxy::nono::capture::{sources_for, MoorCapture, UserSecretsFn};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};

/// A started nono and the internal token it checks.
struct Running {
    /// The proxy.
    proxy: NonoProxy,
    /// The token the listener presents in place of the lease.
    internal: Secret,
}

/// The nonos of every user a lease has used.
pub(crate) struct Users {
    /// Reads a user's secret.
    ledger: Arc<FrontsLedger>,
    /// The CA every front trusts.
    ca: Arc<ca::BoxCa>,
    /// Holds each nono's CA bundle dir.
    state_dir: std::path::PathBuf,
    /// Extra upstream root PEM; `None` in production.
    upstream_ca: Option<std::path::PathBuf>,
    /// The nonos by front and user.
    running: Mutex<BTreeMap<(String, UserId), Running>>,
}

impl Users {
    /// No nono yet; each starts with `ledger`'s secrets and `ca`, keeps its files in a state dir
    /// and trusts an extra upstream root PEM when one is given.
    pub(crate) fn new(
        ledger: Arc<FrontsLedger>,
        ca: Arc<ca::BoxCa>,
        (state_dir, upstream_ca): (std::path::PathBuf, Option<std::path::PathBuf>),
    ) -> Self {
        Self {
            ledger,
            ca,
            state_dir,
            upstream_ca,
            running: Mutex::new(BTreeMap::new()),
        }
    }

    /// The address and internal token of front `ns`'s nono for `user`, started with `rules`
    /// when it is not running; none when it cannot start.
    pub(crate) fn nono(
        &self,
        ns: &str,
        user: UserId,
        rules: &[ResolvedRule],
    ) -> Option<(SocketAddr, Secret)> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        let it = match running.entry((ns.to_owned(), user)) {
            Entry::Occupied(found) => found.into_mut(),
            Entry::Vacant(free) => free.insert(self.start(ns, user, rules)?),
        };
        let reached = (
            SocketAddr::from(([127, 0, 0, 1], it.proxy.port())),
            Secret::new(it.internal.expose().to_vec()),
        );
        drop(running);
        Some(reached)
    }

    /// Starts front `ns`'s nono for `user`.
    fn start(&self, ns: &str, user: UserId, rules: &[ResolvedRule]) -> Option<Running> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).ok()?;
        let internal = Secret::new(crate::proxy::token::base64url(&bytes).into_bytes());
        let dir = super::ca_dir(&self.state_dir).join(format!("{ns}-{}", user.get()));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .ok()?;
        let cfg = build_config(&NonoSpec {
            rules,
            token: &internal,
            ca: &self.ca,
            ca_dir: &dir,
            upstream_ca: self.upstream_ca.as_deref(),
            chain: None,
            only_rules: true,
        });
        let (ledger, front) = (Arc::clone(&self.ledger), ns.to_owned());
        let secrets: UserSecretsFn =
            Arc::new(move |name| ledger.secret(&front, user, name).map_err(|e| e.to_string()));
        let sources = sources_for(rules, &secrets);
        let proxy = NonoProxy::start(cfg, Arc::new(MoorCapture::new(sources))).ok()?;
        Some(Running { proxy, internal })
    }

    /// Stops every nono.
    pub(crate) fn stop(&self) {
        let running =
            std::mem::take(&mut *self.running.lock().unwrap_or_else(PoisonError::into_inner));
        for it in running.into_values() {
            it.proxy.stop();
        }
    }
}
