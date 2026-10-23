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

use super::Deps;
use crate::proxy::nono::capture::{sources_for, MoorCapture, SecretsFn};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};
use crate::vault::Vault;

/// A started nono and the internal token it checks.
struct Running {
    /// The proxy.
    proxy: NonoProxy,
    /// The token the listener presents in place of the lease.
    internal: Secret,
}

/// The nonos of every user a lease has used.
pub(super) struct Users {
    /// Reads a user's secret.
    secrets: SecretsFn,
    /// Opens the secrets.
    vault: Arc<Vault>,
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
    /// No nono yet; each starts with `deps`'s secrets and `ca`.
    pub(super) fn new(deps: &Deps, ca: Arc<ca::BoxCa>) -> Self {
        Self {
            secrets: Arc::clone(&deps.secrets),
            vault: Arc::clone(&deps.vault),
            ca,
            state_dir: deps.state_dir.clone(),
            upstream_ca: deps.upstream_ca.clone(),
            running: Mutex::new(BTreeMap::new()),
        }
    }

    /// The address and internal token of front `ns`'s nono for `user`, started with `rules`
    /// when it is not running; none when it cannot start.
    pub(super) fn nono(
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
        let internal = Secret::new(crate::proxy::detached::base64url(&bytes).into_bytes());
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
            only_rule_hosts: true,
        });
        let sources = sources_for(rules, &self.secrets, &self.vault, user);
        let proxy = NonoProxy::start(cfg, Arc::new(MoorCapture::new(sources))).ok()?;
        Some(Running { proxy, internal })
    }

    /// Stops every nono.
    pub(super) fn stop(&self) {
        let running =
            std::mem::take(&mut *self.running.lock().unwrap_or_else(PoisonError::into_inner));
        for it in running.into_values() {
            it.proxy.stop();
        }
    }
}
