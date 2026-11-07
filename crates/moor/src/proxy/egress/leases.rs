//! Each remote front's leases on the shared egress Proxy, and the proxy each lease's user gets.
//!
//! A lease is looked up on the ledger on every new tunnel, so a released one is refused at once.
//! While the controller cannot be reached, the last answer for a lease is used, so a turn in
//! flight outlives a controller restart.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::proxy::ResolvedRule;
use moor_core::Secret;

use super::boxes::{Boxes, Refused};
use super::served::lock;
use super::Config;
use crate::proxy::fronts::ca_dir;
use crate::proxy::fronts::users::Users;
use crate::proxy::ledger::{FrontLease, FrontsLedger, LedgerError};
use crate::proxy::nono::ca;

/// How long a lease this replica already confirmed waits on the ledger before its last answer is
/// used.
const CACHED_LOOKUP: Duration = Duration::from_secs(1);

/// Most leases remembered for a controller's absence; a turn holds one, so this is far above any
/// that are live at once.
const REMEMBERED: usize = 1024;

/// The leases this replica has served.
pub(super) struct Leases {
    /// Lease lookups, secrets and the CA.
    ledger: Arc<FrontsLedger>,
    /// Where each proxy keeps its files.
    work_dir: PathBuf,
    /// Extra upstream root PEM; `None` in production.
    upstream_ca: Option<PathBuf>,
    /// The rules a lease may name.
    rules: Vec<ResolvedRule>,
    /// Each user's proxy, once the CA has been read.
    users: Mutex<Option<Arc<Users>>>,
    /// The last answer for each lease hash.
    last: Mutex<HashMap<String, FrontLease>>,
}

impl Leases {
    /// None served yet.
    pub(super) fn new(cfg: &Config) -> Self {
        Self {
            ledger: Arc::new(FrontsLedger::egress(
                cfg.ledger.clone(),
                cfg.key.to_string(),
            )),
            work_dir: cfg.work_dir.clone(),
            upstream_ca: cfg.upstream_ca.clone(),
            rules: cfg.front_rules.clone(),
            users: Mutex::default(),
            last: Mutex::default(),
        }
    }

    /// The proxy of the lease whose token hashes to `token_hash` and the token it checks, started
    /// on first use. A lookup spends from `boxes`'s shared budget.
    pub(super) fn admit(
        &self,
        token_hash: &str,
        boxes: &Boxes,
    ) -> Result<(SocketAddr, Secret), Refused> {
        boxes.spend(token_hash)?;
        let lease = self.lease(token_hash)?;
        let rules: Vec<ResolvedRule> = self
            .rules
            .iter()
            .filter(|rule| lease.rules.contains(&rule.name))
            .cloned()
            .collect();
        self.users()
            .map_err(Refused::Unavailable)?
            .nono(&lease.front, lease.user, &rules)
            .ok_or_else(|| Refused::Unavailable("the front's proxy did not start".to_owned()))
    }

    /// The lease of `token_hash`, or the last answer while the controller is away.
    fn lease(&self, token_hash: &str) -> Result<FrontLease, Refused> {
        let cached = lock(&self.last).contains_key(token_hash);
        let answer = if cached {
            self.lookup_within(token_hash)
        } else {
            self.ledger.lease_of(token_hash)
        };
        let mut last = lock(&self.last);
        match answer {
            Ok(Some(lease)) => {
                if last.len() >= REMEMBERED {
                    last.clear();
                }
                last.insert(token_hash.to_owned(), lease.clone());
                Ok(lease)
            }
            Ok(None) => {
                last.remove(token_hash);
                Err(Refused::Unknown)
            }
            Err(e @ LedgerError::Unavailable(_)) => last
                .get(token_hash)
                .cloned()
                .ok_or_else(|| Refused::Unavailable(e.to_string())),
            Err(e) => Err(Refused::Unavailable(e.to_string())),
        }
    }

    /// The ledger's answer for `token_hash`, or unavailable once [`CACHED_LOOKUP`] passes, so a
    /// dying controller costs a lease in use no more than that.
    fn lookup_within(&self, token_hash: &str) -> Result<Option<FrontLease>, LedgerError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ledger, hash) = (Arc::clone(&self.ledger), token_hash.to_owned());
        std::thread::spawn(move || drop(tx.send(ledger.lease_of(&hash))));
        rx.recv_timeout(CACHED_LOOKUP).unwrap_or_else(|_| {
            Err(LedgerError::Unavailable(format!(
                "no answer within {}ms",
                CACHED_LOOKUP.as_millis()
            )))
        })
    }

    /// The users' proxies, with the CA read from the ledger on first use.
    fn users(&self) -> Result<Arc<Users>, String> {
        let mut users = lock(&self.users);
        if let Some(users) = users.as_ref() {
            return Ok(Arc::clone(users));
        }
        let pair = self
            .ledger
            .ca()
            .map_err(|e| format!("cannot read the front CA: {e}"))?;
        let installed = ca::install_in(
            &ca_dir(&self.work_dir),
            (&pair.key_pem, pair.cert_pem.as_str()),
        )
        .map_err(|e| format!("cannot install the front CA: {e}"))?;
        let made = Arc::new(Users::new(
            Arc::clone(&self.ledger),
            Arc::new(installed),
            (self.work_dir.clone(), self.upstream_ca.clone()),
        ));
        *users = Some(Arc::clone(&made));
        drop(users);
        Ok(made)
    }

    /// Stops every proxy.
    pub(super) fn stop(&self) {
        let users = lock(&self.users).take();
        if let Some(users) = users {
            users.stop();
        }
    }
}
