//! Spots a credential rule's account being rate limited or rejected, and recovering.
//!
//! Only the rule name and the status are read. Each status is reported once per
//! rule until a 2xx shows the rule's upstream answering again.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::domain::event::Event;
use nono::undo::NetworkAuditEvent;

use super::{bare_host, rule, EgressSink};

/// Statuses already reported for each rule since its last 2xx.
#[derive(Default)]
pub(crate) struct Limits {
    /// Open statuses by rule; a rule seeded from the store appears once it is first seen.
    open: Mutex<BTreeMap<String, BTreeSet<u16>>>,
    /// 401s and 403s that went to requests forwarded without the credential
    /// nono injected, and not yet audited.
    anonymous: Mutex<Notes>,
}

/// Counts of noted answers by lower-case host and status.
type Noted = BTreeMap<(String, u16), usize>;

/// Noted answers by how many drains have ended since they were noted.
#[derive(Default)]
struct Notes {
    /// Noted since the last drain ended.
    fresh: Noted,
    /// Noted before the last drain ended; dropped when the next one ends, so
    /// an answer nono never audits cannot hide a later rejection.
    aged: Noted,
}

impl Notes {
    /// Consumes one note for `key`, the oldest first.
    fn take(&mut self, key: &(String, u16)) -> bool {
        take_one(&mut self.aged, key) || take_one(&mut self.fresh, key)
    }
}

/// Consumes one count of `key` from `noted`.
fn take_one(noted: &mut Noted, key: &(String, u16)) -> bool {
    let Some(count) = noted.get_mut(key) else {
        return false;
    };
    *count -= 1;
    if *count == 0 {
        noted.remove(key);
    }
    true
}

impl Limits {
    /// Notes that the next audited `status` from `host` answered a request
    /// that was forwarded without its credential.
    pub(super) fn anonymous(&self, host: &str, status: u16) {
        *self
            .lock_anonymous()
            .fresh
            .entry((host.to_ascii_lowercase(), status))
            .or_default() += 1;
    }

    /// Consumes one noted anonymous answer matching `event`, if any.
    fn take_anonymous(&self, event: &NetworkAuditEvent, status: u16) -> bool {
        let key = (bare_host(&event.target).to_ascii_lowercase(), status);
        self.lock_anonymous().take(&key)
    }

    /// Ends a drain: drops the notes no audit consumed within one further
    /// drain, and ages the rest.
    pub(super) fn end_drain(&self) {
        let mut notes = self.lock_anonymous();
        notes.aged = mem::take(&mut notes.fresh);
    }

    /// Locks the noted anonymous answers, recovering from poisoning.
    fn lock_anonymous(&self) -> MutexGuard<'_, Notes> {
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
