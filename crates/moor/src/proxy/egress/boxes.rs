//! Each token's Box, and the proxy this replica runs for each Box it serves.
//!
//! A token is looked up on the ledger on every new tunnel, so a Box that closed is refused at
//! once, and a Box whose spec changed gets a proxy built from the new spec before that tunnel
//! starts. Only tokens the ledger confirmed are kept, each with its tunnel bucket and count. While
//! the controller cannot be reached, the last answer for a token is used, as a Box's own Proxy
//! uses its last answers.

#[cfg(test)]
mod inspect;
pub(super) mod warm;

use std::collections::HashMap;
use std::fmt::Arguments;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::seam::clock::Clock;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::BoxId;
use zeroize::Zeroizing;

use super::limit::{Bucket, Rate};
use super::served::{lock, Served, Tunnel};
use super::Config;
use crate::proxy::engine::{engine, Parts};
use crate::proxy::ledger::{BoxLedger, BoxView, EgressLedger, Endpoint, LedgerError, Owner};
use crate::proxy::nono::ca;
use crate::proxy::nono::capture::UserSecretsFn;
use crate::proxy::{token, Sink};

/// Where a tunnel for a Box goes; holding it keeps the proxy and counts the tunnel.
pub(super) struct Route {
    /// The Box's proxy.
    pub(super) served: Arc<Served>,
    /// The token's count of open tunnels.
    _tunnel: Tunnel,
}

/// Why a token gets no route.
pub(super) enum Refused {
    /// The token is unknown or its Box closed.
    Unknown,
    /// The token's rate or tunnel count, or the shared lookup budget, is spent.
    Limited,
    /// The controller cannot be reached and the token was never seen, or the Box's proxy could
    /// not start; the reason is for the log only.
    Unavailable(String),
}

/// How long a token this replica already confirmed waits on the ledger before its last answer
/// is used.
const CACHED_LOOKUP: Duration = Duration::from_secs(1);

/// A token the ledger confirmed.
struct Known {
    /// Its Box.
    box_id: BoxId,
    /// Its tunnel bucket.
    bucket: Bucket,
    /// Its open tunnels.
    open: Arc<AtomicUsize>,
}

/// A Box's proxy and the spec digest it last followed.
struct Held {
    /// The proxy new tunnels go to.
    served: Arc<Served>,
    /// The controller's spec digest this proxy was last checked against; none when unknown.
    followed: Option<String>,
}

/// The proxy of one Box, replaced when its spec changes.
#[derive(Default)]
struct Slot {
    /// The proxy new tunnels go to.
    held: Mutex<Option<Held>>,
    /// Held while a proxy of the Box is built or its CA removed, both of which touch the Box's
    /// CA directory; true once the Box's proxy is closing, when no proxy of it is built again.
    building: Mutex<bool>,
}

/// Every Box this replica serves.
pub(super) struct Boxes {
    /// Token lookups.
    lookup: Arc<EgressLedger>,
    /// Where each Box's ledger calls go.
    ledger: Endpoint,
    /// The egress key, which also signs tokens.
    key: Zeroizing<String>,
    /// Where each Box's CA and log go.
    work_dir: PathBuf,
    /// Extra upstream root PEM; `None` in production.
    upstream_ca: Option<PathBuf>,
    /// Loopback addresses the egress guard lets through.
    guard_exempt: Vec<SocketAddr>,
    /// Stamps the request logs and fills the buckets.
    clock: Arc<dyn Clock>,
    /// Each confirmed token's rate.
    token_rate: Rate,
    /// Most tunnels a token may hold open at once.
    tunnels: usize,
    /// The rate of lookups for tokens not yet confirmed, shared by all of them.
    lookup_rate: Rate,
    /// The shared lookup bucket.
    lookups: Mutex<Bucket>,
    /// Each confirmed token hash.
    known: Mutex<HashMap<String, Known>>,
    /// Each Box's slot; the map lock is never held across a ledger call or a proxy start.
    served: Mutex<HashMap<BoxId, Arc<Slot>>>,
    /// Where failures are logged.
    log: Sink,
    /// Whether the ledger has answered a [`Boxes::warm`] since this replica started.
    synced: AtomicBool,
}

impl Boxes {
    /// No Box served yet.
    pub(super) fn new(cfg: Config) -> Self {
        Self {
            lookup: Arc::new(EgressLedger::at(cfg.ledger.clone(), cfg.key.to_string())),
            ledger: cfg.ledger,
            key: cfg.key,
            work_dir: cfg.work_dir,
            upstream_ca: cfg.upstream_ca,
            guard_exempt: cfg.guard_exempt,
            token_rate: cfg.rate,
            tunnels: cfg.tunnels,
            lookup_rate: cfg.lookups,
            lookups: Mutex::new(Bucket::new(cfg.lookups.burst, cfg.clock.instant())),
            clock: cfg.clock,
            known: Mutex::default(),
            served: Mutex::default(),
            log: cfg.log,
            synced: AtomicBool::new(false),
        }
    }

    /// Whether `token` was signed with this replica's egress key.
    pub(super) fn signed(&self, token: &[u8]) -> bool {
        token::verify(self.key.as_bytes(), token)
    }

    /// The route for the Box whose token hashes to `token_hash`, starting its proxy on first use.
    pub(super) fn admit(&self, token_hash: &str) -> Result<Route, Refused> {
        self.spend(token_hash)?;
        let (box_id, spec) = self.owner(token_hash)?;
        let open = lock(&self.known)
            .get(token_hash)
            .map(|k| Arc::clone(&k.open))
            .ok_or(Refused::Unknown)?;
        let tunnel = Tunnel::claim(&open, self.tunnels).ok_or(Refused::Limited)?;
        Ok(Route {
            served: self
                .served(box_id, spec.as_deref())
                .map_err(Refused::Unavailable)?,
            _tunnel: tunnel,
        })
    }

    /// The proxy of `box_id`, started on first use. When `spec`, the controller's spec digest,
    /// is not the one the proxy last followed, the proxy is first rebuilt if the spec changed.
    fn served(&self, box_id: BoxId, spec: Option<&str>) -> Result<Arc<Served>, String> {
        let slot = Arc::clone(lock(&self.served).entry(box_id).or_default());
        let mut current = lock(&slot.held);
        let mut replaced = None;
        let served = if let Some(held) = current.as_mut() {
            if spec.is_some() && held.followed.as_deref() != spec {
                replaced = self
                    .rebuilt(&slot, box_id, &held.served)
                    .map(|fresh| std::mem::replace(&mut held.served, Arc::new(fresh)));
                held.followed = spec.map(str::to_owned);
            }
            Arc::clone(&held.served)
        } else {
            let started = Arc::new(self.start(&slot, box_id, None)?);
            *current = Some(Held {
                served: Arc::clone(&started),
                followed: spec.map(str::to_owned),
            });
            started
        };
        drop(current);
        // A replaced proxy delivers its audit as it stops, which never holds up the slot.
        drop(replaced);
        Ok(served)
    }

    /// Takes one tunnel from the bucket of `token_hash`, or from the shared lookup bucket when the
    /// token is not confirmed yet.
    pub(super) fn spend(&self, token_hash: &str) -> Result<(), Refused> {
        let now = self.clock.instant();
        let mut known = lock(&self.known);
        let allowed = match known.get_mut(token_hash) {
            Some(k) => k.bucket.take(self.token_rate, now),
            None => lock(&self.lookups).take(self.lookup_rate, now),
        };
        drop(known);
        allowed.then_some(()).ok_or(Refused::Limited)
    }

    /// The open Box of `token_hash` and its spec digest, or the last answer, with no digest,
    /// while the controller is away.
    fn owner(&self, token_hash: &str) -> Result<(BoxId, Option<String>), Refused> {
        let cached = lock(&self.known).contains_key(token_hash);
        let answer = if cached {
            self.lookup_within(token_hash, CACHED_LOOKUP)
        } else {
            self.lookup.token_spec(token_hash)
        };
        let mut known = lock(&self.known);
        match answer {
            Ok(Some(Owner { box_id, spec })) => {
                // The first tunnel counts against the token's own bucket too.
                let first = (self.token_rate.burst - 1.0).max(0.0);
                let now = self.clock.instant();
                known
                    .entry(token_hash.to_owned())
                    .or_insert_with(|| Known {
                        box_id,
                        bucket: Bucket::new(first, now),
                        open: Arc::default(),
                    })
                    .box_id = box_id;
                Ok((box_id, spec))
            }
            Ok(None) => {
                known.remove(token_hash);
                Err(Refused::Unknown)
            }
            Err(e @ LedgerError::Unavailable(_)) => known
                .get(token_hash)
                .map(|k| (k.box_id, None))
                .ok_or_else(|| Refused::Unavailable(e.to_string())),
            Err(e) => Err(Refused::Unavailable(e.to_string())),
        }
    }

    /// The ledger's answer for `token_hash`, or unavailable once `wait` passes, so a dying
    /// controller costs a cached token no more than `wait`.
    fn lookup_within(
        &self,
        token_hash: &str,
        wait: Duration,
    ) -> Result<Option<Owner>, LedgerError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (lookup, hash) = (Arc::clone(&self.lookup), token_hash.to_owned());
        std::thread::spawn(move || drop(tx.send(lookup.token_spec(&hash))));
        rx.recv_timeout(wait).unwrap_or_else(|_| {
            Err(LedgerError::Unavailable(format!(
                "no answer within {}ms",
                wait.as_millis()
            )))
        })
    }

    /// Starts the proxy of `box_id` from `spec`, or from the spec the controller keeps, once no
    /// other proxy of the Box is being built.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the build holds the CA directory"
    )]
    fn start(&self, slot: &Slot, box_id: BoxId, spec: Option<ProxySpec>) -> Result<Served, String> {
        let closing = lock(&slot.building);
        if *closing {
            return Err(format!("Box {} is closing", box_id.get()));
        }
        let ledger = Arc::new(BoxLedger::egress(
            self.ledger.clone(),
            box_id,
            self.key.to_string(),
        ));
        let spec = match spec {
            Some(spec) => spec,
            None => ledger
                .spec()
                .map_err(|e| format!("cannot read the proxy spec: {e}"))?
                .ok_or_else(|| format!("no proxy spec for Box {}", box_id.get()))?,
        };
        let reads = Arc::clone(&ledger);
        let secrets: UserSecretsFn =
            Arc::new(move |name, repo| reads.secret(name, repo).map_err(|e| e.to_string()));
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
        Ok(Served::new(ledger, spec, engine))
    }

    /// Delivers each Box's audit, stops the proxy of each Box that closed and rebuilds the proxy
    /// of each Box whose spec changed.
    pub(super) fn tend(&self) {
        let slots: Vec<(BoxId, Arc<Slot>)> = lock(&self.served)
            .iter()
            .map(|(id, slot)| (*id, Arc::clone(slot)))
            .collect();
        for (box_id, slot) in slots {
            let Some(served) = lock(&slot.held).as_ref().map(|h| Arc::clone(&h.served)) else {
                continue;
            };
            served.drain();
            let view = served.ledger.view();
            if let Err(e) = &view {
                self.say(format_args!("cannot read Box {}: {e}", box_id.get()));
            }
            if closed(&view) {
                self.close(box_id, &slot, &served);
            } else if view.is_ok() {
                self.tend_spec(box_id, &slot, &served);
            }
        }
    }

    /// Removes the CA of a Box that closed, stops its proxy and forgets its tokens. The slot
    /// stays in the map, refusing builds, until the CA is gone, so no build of the Box overlaps
    /// the removal.
    fn close(&self, box_id: BoxId, slot: &Arc<Slot>, served: &Served) {
        let mut closing = lock(&slot.building);
        *closing = true;
        if let Err(e) = ca::remove(&self.work_dir, box_id) {
            self.say(format_args!("cannot remove Box {}'s CA: {e}", box_id.get()));
        }
        drop(closing);
        lock(&slot.held).take();
        served.stop();
        lock(&self.known).retain(|_, k| k.box_id != box_id);
        lock(&self.served).remove(&box_id);
    }

    /// Swaps in [`Boxes::rebuilt`] unless a tunnel already replaced `served`. The ledger is read
    /// before the slot is locked, so a silent controller holds up only a tunnel that needs a
    /// rebuild too. A proxy it rebuilds has followed no digest, so the next tunnel checks it once
    /// more.
    fn tend_spec(&self, box_id: BoxId, slot: &Slot, served: &Arc<Served>) {
        let Some(fresh) = self.rebuilt(slot, box_id, served) else {
            return;
        };
        let fresh = Arc::new(fresh);
        let mut current = lock(&slot.held);
        let unused = match current.as_mut() {
            Some(held) if Arc::ptr_eq(&held.served, served) => {
                held.followed = None;
                std::mem::replace(&mut held.served, fresh)
            }
            _ => fresh,
        };
        drop(current);
        drop(unused);
    }

    /// A proxy built from the spec the controller holds for `box_id`, when it is not the spec
    /// `served` runs. A failed read or rebuild keeps `served` and is only logged.
    fn rebuilt(&self, slot: &Slot, box_id: BoxId, served: &Served) -> Option<Served> {
        match served.ledger.spec() {
            Ok(Some(spec)) if spec != served.spec => self
                .start(slot, box_id, Some(spec))
                .map_err(|e| self.say(format_args!("cannot rebuild Box {}: {e}", box_id.get())))
                .ok(),
            Ok(_) => None,
            Err(e) => {
                self.say(format_args!("cannot read Box {}'s spec: {e}", box_id.get()));
                None
            }
        }
    }

    /// Delivers what audit is left and stops every proxy.
    pub(super) fn stop(&self) {
        let slots: Vec<Arc<Slot>> = lock(&self.served).drain().map(|(_, s)| s).collect();
        for slot in slots {
            let taken = lock(&slot.held).take();
            if let Some(held) = taken {
                held.served.stop();
            }
        }
    }

    /// Writes one `moor egress: ` line to the log.
    pub(super) fn say(&self, args: Arguments<'_>) {
        drop(writeln!(
            self.log.lock().unwrap_or_else(PoisonError::into_inner),
            "moor egress: {args}"
        ));
    }
}

/// Whether `view` says the Box closed; a failed read never does.
pub(super) const fn closed(view: &Result<Option<BoxView>, LedgerError>) -> bool {
    matches!(view, Ok(None)) || matches!(view, Ok(Some(v)) if !v.open)
}
