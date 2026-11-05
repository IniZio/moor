//! The shared egress Proxy's ledger for one Box: the reads a request makes and its audit.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use moor_core::domain::event::Event;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::SecretName;
use moor_core::{BoxHandle, BoxId, Secret};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::client::Ledger;
use super::{Call, Endpoint, Hello, LedgerError, LEDGER};
use crate::proxy::front::Hold;

/// Audit events kept while the controller is unreachable; past it the oldest go.
const PENDING: usize = 256;

/// The Box as the shared egress Proxy sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BoxView {
    /// Work can still happen on it: it is neither closed nor rejected.
    pub(crate) open: bool,
    /// Its machine, when it has one.
    pub(crate) handle: Option<BoxHandle>,
    /// It is stored as asleep.
    pub(crate) asleep: bool,
}

/// A Box's interception CA as the ledger hands it to the Box's engine.
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

/// The last answers the controller gave to the reads a request makes, served while it is
/// unavailable. A secret revoked meanwhile stays in use until the controller answers again.
#[derive(Default)]
struct LastAnswers {
    /// Each secret by name, `None` when the user had none.
    secrets: BTreeMap<SecretName, Option<Zeroizing<Vec<u8>>>>,
    /// What the Box held.
    hold: Option<Hold>,
}

/// The ledger of one Box the shared egress Proxy serves.
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
    #[cfg(test)]
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
}
