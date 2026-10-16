//! Spots a credential rule's account being rate limited or rejected, and recovering.
//!
//! Only the rule name and the status are read. Each status is reported once per
//! rule until a 2xx shows the rule's upstream answering again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use moor_core::domain::event::Event;
use nono::undo::NetworkAuditEvent;

use super::{rule, EgressSink};

/// Statuses already reported for each rule since its last 2xx.
#[derive(Default)]
pub(crate) struct Limits {
    /// Open statuses by rule; a rule seeded from the store appears once it is first seen.
    open: Mutex<BTreeMap<String, BTreeSet<u16>>>,
}

impl Limits {
    /// Reports to `sink` the limit change `event` shows, if any.
    pub(super) fn observe(&self, event: &NetworkAuditEvent, sink: &dyn EgressSink) {
        let (Some(rule), Some(status)) = (rule(event), event.status) else {
            return;
        };
        let rejected =
            matches!(status, 401 | 403) && event.managed_credential_active != Some(false);
        if status != 429 && !rejected && !(200..300).contains(&status) {
            return;
        }
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
