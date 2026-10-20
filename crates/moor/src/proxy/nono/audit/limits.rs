//! Spots a credential rule's account being rate limited or rejected, and recovering.
//!
//! Only the rule name and the status are read. Each status is reported once per
//! rule until a 2xx shows the rule's upstream answering again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::domain::event::Event;
use nono::undo::NetworkAuditEvent;

use super::{bare_host, rule, EgressSink};

/// Statuses already reported for each rule since its last 2xx.
#[derive(Default)]
pub(crate) struct Limits {
    /// Open statuses by rule; a rule seeded from the store appears once it is first seen.
    open: Mutex<BTreeMap<String, BTreeSet<u16>>>,
    /// 401s and 403s, by lower-case host and status, that went to requests
    /// forwarded without the credential nono injected, and not yet audited.
    anonymous: Mutex<BTreeMap<(String, u16), usize>>,
}

impl Limits {
    /// Notes that the next audited `status` from `host` answered a request
    /// that was forwarded without its credential.
    pub(super) fn anonymous(&self, host: &str, status: u16) {
        *self
            .lock_anonymous()
            .entry((host.to_ascii_lowercase(), status))
            .or_default() += 1;
    }

    /// Consumes one noted anonymous answer matching `event`, if any.
    fn take_anonymous(&self, event: &NetworkAuditEvent, status: u16) -> bool {
        let key = (bare_host(&event.target).to_ascii_lowercase(), status);
        let mut anonymous = self.lock_anonymous();
        let Some(count) = anonymous.get_mut(&key) else {
            return false;
        };
        *count -= 1;
        if *count == 0 {
            anonymous.remove(&key);
        }
        true
    }

    /// Locks the noted anonymous answers, recovering from poisoning.
    fn lock_anonymous(&self) -> MutexGuard<'_, BTreeMap<(String, u16), usize>> {
        self.anonymous
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Reports to `sink` the limit change `event` shows, if any.
    pub(super) fn observe(&self, event: &NetworkAuditEvent, sink: &dyn EgressSink) {
        let (Some(rule), Some(status)) = (rule(event), event.status) else {
            return;
        };
        let rejected = matches!(status, 401 | 403)
            && event.managed_credential_active != Some(false)
            && !self.take_anonymous(event, status);
        if status != 429 && !rejected && !(200..300).contains(&status) {
            return;
        }
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let statuses = open
            .entry(rule.to_owned())
            .or_insert_with(|| sink.open_limit(rule).into_iter().collect());
        let reported = decide(statuses, rule, status);
        drop(open);
        if let Some(reported) = reported {
            sink.limit(&reported);
        }
    }
}

/// The event to record for `status` on `rule`, updating the rule's open `statuses`.
fn decide(statuses: &mut BTreeSet<u16>, rule: &str, status: u16) -> Option<Event> {
    match status {
        429 => statuses.insert(status).then(|| Event::AgentRateLimited {
            rule: rule.to_owned(),
            status,
            resets_at: None,
        }),
        401 | 403 => statuses
            .insert(status)
            .then(|| Event::ProxyCredentialRejected {
                rule: rule.to_owned(),
                status,
            }),
        _ => {
            let was_open = !statuses.is_empty();
            statuses.clear();
            was_open.then(|| Event::ProxyCredentialRecovered {
                rule: rule.to_owned(),
            })
        }
    }
}
