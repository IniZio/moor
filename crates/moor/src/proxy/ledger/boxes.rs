//! A Box supervisor's ledger: its store calls, each for the Box its key was minted for.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::domain::event::{Event, MachineEvent};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::SecretName;
use moor_core::{BoxHandle, BoxId, Secret};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::client::Ledger;
use super::{Call, Endpoint, Hello, LedgerError, LEDGER};
use crate::ledger_dir;
use crate::proxy::front::Hold;
use crate::store::{Fenced, Lease, MachineWake, ReadyProc, TunnelAck, TunnelRequest};

/// Audit events kept while the controller is unreachable; past it the oldest go.
const PENDING: usize = 256;

/// The Box as its supervisor sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BoxView {
    /// Work can still happen on it: it is neither closed nor rejected.
    pub(crate) open: bool,
    /// Its machine, when it has one.
    pub(crate) handle: Option<BoxHandle>,
    /// It is stored as asleep.
    pub(crate) asleep: bool,
}

/// A Box's interception CA as the ledger hands it to the Box's supervisor.
#[derive(Serialize, Deserialize)]
pub(crate) struct CaPair {
    /// The PEM private key, zeroized on drop.
    pub(crate) key_pem: Vec<u8>,
    /// The PEM certificate the Box trusts.
    pub(crate) cert_pem: String,
}

impl Drop for CaPair {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.key_pem);
    }
}

/// The machine a Box holds a slot on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MachineWant {
    /// The machine's name.
    pub(crate) name: String,
    /// The forwarded ports.
    pub(crate) ports: Vec<u16>,
    /// Its enrolled id, which its agent links in under.
    pub(crate) machine_id: i64,
}

/// The last answers the controller gave to the reads a request makes, served while it is
/// unavailable. A secret revoked meanwhile stays in use until the controller answers again.
#[derive(Default)]
struct LastAnswers {
    /// Each secret by name, `None` when the user had none.
    secrets: BTreeMap<SecretName, Option<Zeroizing<Vec<u8>>>>,
    /// What the Box held.
    hold: Option<Hold>,
}

/// The ledger of one Box's supervisor.
pub(crate) struct BoxLedger {
    /// The Box it calls for.
    box_id: BoxId,
    /// The connections.
    ledger: Ledger,
    /// Audit events not yet delivered, oldest first.
    pending: Mutex<VecDeque<Event>>,
    /// Audit events dropped since last asked.
    dropped: AtomicUsize,
    /// What the reads a request makes last returned.
    last: Mutex<LastAnswers>,
}

impl BoxLedger {
    /// The ledger of the controller whose state dir is `state_dir`, for `box_id`, presenting `key`.
    pub(crate) fn new(state_dir: &Path, box_id: BoxId, key: String) -> Self {
        Self::at(ledger_dir::socket(state_dir), box_id, key)
    }

    /// The ledger on `socket`, for `box_id`, presenting `key`.
    pub(crate) fn at(socket: PathBuf, box_id: BoxId, key: String) -> Self {
        let hello = Hello::Box {
            v: LEDGER,
            box_id,
            key,
        };
        Self::with_hello(box_id, Endpoint::Unix(socket), &hello)
    }

    /// The shared egress Proxy's ledger at `endpoint` for `box_id`, presenting its `key`; the
    /// controller answers only the reads a request makes and its audit.
    pub(crate) fn egress(endpoint: Endpoint, box_id: BoxId, key: String) -> Self {
        let hello = Hello::Egress {
            v: LEDGER,
            key,
            box_id: Some(box_id),
        };
        Self::with_hello(box_id, endpoint, &hello)
    }

    /// The ledger at `endpoint` for `box_id`, opening each connection with `hello`.
    fn with_hello(box_id: BoxId, endpoint: Endpoint, hello: &Hello) -> Self {
        Self {
            box_id,
            ledger: Ledger::new(endpoint, hello),
            pending: Mutex::new(VecDeque::new()),
            dropped: AtomicUsize::new(0),
            last: Mutex::default(),
        }
    }

    /// The Box it calls for.
    pub(crate) const fn box_id(&self) -> BoxId {
        self.box_id
    }

    /// The Box; none when it is gone.
    pub(crate) fn view(&self) -> Result<Option<BoxView>, LedgerError> {
        self.ledger.call(&Call::View)
    }

    /// The proxy row's Box port; none when the row is gone.
    pub(crate) fn box_port(&self) -> Result<Option<u16>, LedgerError> {
        self.ledger.call(&Call::BoxPort)
    }

    /// Takes the lease for `owner` when it is free, already `owner`'s, or older than `lease`.
    pub(crate) fn claim_lease(&self, owner: &str, lease: Duration) -> Result<Lease, LedgerError> {
        let lease_ms = u64::try_from(lease.as_millis()).unwrap_or(u64::MAX);
        self.ledger.call(&Call::ClaimLease {
            owner: owner.to_owned(),
            lease_ms,
        })
    }

    /// Extends `owner`'s lease; false when another holds it.
    pub(crate) fn heartbeat(&self, owner: &str) -> Result<bool, LedgerError> {
        self.ledger.call(&Call::Heartbeat {
            owner: owner.to_owned(),
        })
    }

    /// Gives up `owner`'s lease.
    pub(crate) fn release(&self, owner: &str) -> Result<(), LedgerError> {
        self.ledger.call(&Call::Release {
            owner: owner.to_owned(),
        })
    }

    /// Marks the proxy ready on `box_port` while `owner` holds the lease.
    pub(crate) fn mark_ready(
        &self,
        owner: &str,
        box_port: u16,
        proc: ReadyProc<'_>,
    ) -> Result<Fenced, LedgerError> {
        self.ledger.call(&Call::MarkReady {
            owner: owner.to_owned(),
            box_port,
            pid: proc.pid,
            handle: proc.handle.to_owned(),
            boot_id: proc.boot_id.map(str::to_owned),
        })
    }

    /// Whether a tunnel was ever opened into the Box.
    pub(crate) fn tunnel_opened(&self) -> Result<bool, LedgerError> {
        self.ledger.call(&Call::TunnelOpened)
    }

    /// Records that a tunnel is about to be opened into the Box.
    pub(crate) fn note_tunnel_opened(&self) -> Result<(), LedgerError> {
        self.ledger.call(&Call::NoteTunnelOpened)
    }

    /// The latest tunnel request.
    pub(crate) fn tunnel_request(&self) -> Result<Option<TunnelRequest>, LedgerError> {
        self.ledger.call(&Call::TunnelRequest)
    }

    /// The latest tunnel answer.
    pub(crate) fn tunnel_ack(&self) -> Result<Option<TunnelAck>, LedgerError> {
        self.ledger.call(&Call::TunnelAck)
    }

    /// Records `ack` as the answer to a tunnel request.
    pub(crate) fn ack_tunnel(&self, ack: &TunnelAck) -> Result<(), LedgerError> {
        self.ledger.call(&Call::AckTunnel { ack: ack.clone() })
    }

    /// Records a tunnel restart on `box_port` while `owner` holds the lease.
    pub(crate) fn tunnel_restarted(
        &self,
        owner: &str,
        box_port: u16,
        reason: &str,
    ) -> Result<Fenced, LedgerError> {
        self.ledger.call(&Call::TunnelRestarted {
            owner: owner.to_owned(),
            box_port,
            reason: reason.to_owned(),
        })
    }

    /// Records `event` while `owner` holds the lease.
    pub(crate) fn machine_event(
        &self,
        owner: &str,
        event: &MachineEvent,
    ) -> Result<Fenced, LedgerError> {
        self.ledger.call(&Call::MachineEvent {
            owner: owner.to_owned(),
            event: event.clone(),
        })
    }

    /// The machine the Box holds a slot on.
    pub(crate) fn box_machine(&self) -> Result<Option<MachineWant>, LedgerError> {
        self.ledger.call(&Call::BoxMachine)
    }

    /// What the Box holds, as the machine service reads it on each request; the last answer
    /// while the controller is unavailable.
    pub(crate) fn hold(&self) -> Result<Hold, LedgerError> {
        let answer = self.ledger.call::<Hold>(&Call::Hold);
        let mut last = self.last();
        match answer {
            Ok(hold) => {
                last.hold = Some(hold.clone());
                Ok(hold)
            }
            Err(e @ LedgerError::Unavailable(_)) => last.hold.clone().ok_or(e),
            Err(e) => Err(e),
        }
    }

    /// Frees the Box's slot as it sleeps; the machine's name when it held one.
    pub(crate) fn sleep_machine(&self) -> Result<Option<String>, LedgerError> {
        self.ledger.call(&Call::SleepMachine)
    }

    /// Grants the Box's machine want.
    pub(crate) fn grant_machine(&self) -> Result<MachineWake, LedgerError> {
        self.ledger.call(&Call::GrantMachine)
    }

    /// Whether the newest machine event already says the Box waits at `position`.
    pub(crate) fn waiting_said(&self, position: u32) -> Result<bool, LedgerError> {
        self.ledger.call(&Call::WaitingSaid { position })
    }

    /// Lets go of the Box's machine; true when it held one.
    pub(crate) fn detach_machine(&self) -> Result<bool, LedgerError> {
        self.ledger.call(&Call::DetachMachine)
    }

    /// Records the audit event `event`, after any kept while the controller was unreachable.
    ///
    /// An event the controller refuses is dropped; one it cannot be reached for is kept.
    pub(crate) fn record_event(&self, event: &Event) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.push_back(event.clone());
        while pending.len() > PENDING {
            pending.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.deliver(&mut pending);
        drop(pending);
    }

    /// Delivers the audit events kept while the controller was unreachable.
    pub(crate) fn flush(&self) {
        self.deliver(&mut self.pending.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// How many audit events were dropped since the last call.
    pub(crate) fn dropped(&self) -> usize {
        self.dropped.swap(0, Ordering::Relaxed)
    }

    /// Delivers `pending` oldest first, stopping at the first the controller cannot be reached for.
    fn deliver(&self, pending: &mut VecDeque<Event>) {
        while let Some(event) = pending.front() {
            let call = Call::RecordEvent {
                event: event.clone(),
            };
            if let Err(LedgerError::Unavailable(_)) = self.ledger.call::<()>(&call) {
                return;
            }
            pending.pop_front();
        }
    }

    /// The status of the open limit on credential rule `rule`.
    pub(crate) fn open_limit(&self, rule: &str) -> Result<Option<u16>, LedgerError> {
        self.ledger.call(&Call::OpenLimit {
            rule: rule.to_owned(),
        })
    }

    /// The secret `name` of the Box's user; the last answer while the controller is unavailable.
    pub(crate) fn secret(&self, name: &SecretName) -> Result<Option<Secret>, LedgerError> {
        let answer = self
            .ledger
            .call::<Option<Vec<u8>>>(&Call::Secret { name: name.clone() })
            .map(|bytes| bytes.map(Zeroizing::new));
        let mut last = self.last();
        let bytes = match answer {
            Ok(bytes) => {
                last.secrets.insert(name.clone(), bytes.clone());
                bytes
            }
            Err(e @ LedgerError::Unavailable(_)) => last.secrets.get(name).cloned().ok_or(e)?,
            Err(e) => return Err(e),
        };
        drop(last);
        Ok(bytes.map(|b| Secret::new(b.to_vec())))
    }

    /// The last answers, locked.
    fn last(&self) -> std::sync::MutexGuard<'_, LastAnswers> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The spec the Box's Proxy starts from; none when the controller recorded none.
    pub(crate) fn spec(&self) -> Result<Option<ProxySpec>, LedgerError> {
        self.ledger.call(&Call::Spec)
    }

    /// The Box's interception CA, which the controller makes on the first call.
    pub(crate) fn ca(&self) -> Result<CaPair, LedgerError> {
        self.ledger.call(&Call::Ca)
    }

    /// Removes the proxy row.
    pub(crate) fn remove_proxy(&self) -> Result<bool, LedgerError> {
        self.ledger.call(&Call::RemoveProxy)
    }
}
