//! Spots a credential rule's account being rate limited or rejected, and recovering.
//!
//! The rule name and the status are read, and for a 401 or 403 from the GitHub
//! API the repo the path names. Each status is reported once per rule until a
//! 2xx on a request that carried the rule's credential shows the upstream
//! answering again.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::domain::event::Event;
use nono::undo::NetworkAuditEvent;

use moor_core::seam::forge::ForgeRepo;

use super::{bare_host, rule, EgressSink};
use crate::proxy::nono::GITHUB_API_HOST;

/// Statuses already reported for each rule since its last 2xx.
#[derive(Default)]
pub(crate) struct Limits {
    /// Open statuses by rule; a rule seeded from the store appears once it is first seen.
    open: Mutex<BTreeMap<String, BTreeSet<u16>>>,
    /// 401s and 403s that say nothing about the credential nono injected, and
    /// not yet audited.
    excused: Mutex<Notes>,
    /// The Box's repos; a 401 or 403 from another repo says nothing about the
    /// credential.
    repos: Vec<ForgeRepo>,
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
    /// Adds `repos` to the Box's repos.
    pub(super) fn add_repos(&mut self, repos: impl IntoIterator<Item = ForgeRepo>) {
        self.repos.extend(repos);
    }

    /// Whether `event` is a GitHub API request for a repo outside the Box's repos.
    fn outside_repos(&self, event: &NetworkAuditEvent) -> bool {
        if !bare_host(&event.target).eq_ignore_ascii_case(GITHUB_API_HOST) {
            return false;
        }
        let path = event.path.as_deref().unwrap_or_default();
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let mut segments = path.split('/').filter(|s| !s.is_empty());
        let (Some("repos"), Some(owner), Some(name)) =
            (segments.next(), segments.next(), segments.next())
        else {
            return false;
        };
        !self
            .repos
            .iter()
            .any(|r| r.owner.eq_ignore_ascii_case(owner) && r.name.eq_ignore_ascii_case(name))
    }

    /// Notes that the next audited `status` from `host` says nothing about the
    /// credential.
    pub(super) fn excuse(&self, host: &str, status: u16) {
        *self
            .lock_excused()
            .fresh
            .entry((host.to_ascii_lowercase(), status))
            .or_default() += 1;
    }

    /// Consumes one excused answer matching `event`, if any.
    fn take_excused(&self, event: &NetworkAuditEvent, status: u16) -> bool {
        let key = (bare_host(&event.target).to_ascii_lowercase(), status);
        self.lock_excused().take(&key)
    }

    /// Ends a drain: drops the notes no audit consumed within one further
    /// drain, and ages the rest.
    pub(super) fn end_drain(&self) {
        let mut notes = self.lock_excused();
        notes.aged = mem::take(&mut notes.fresh);
    }

    /// Locks the excused answers, recovering from poisoning.
    fn lock_excused(&self) -> MutexGuard<'_, Notes> {
        self.excused.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reports to `sink` the limit change `event` shows, if any.
    pub(super) fn observe(&self, event: &NetworkAuditEvent, sink: &dyn EgressSink) {
        let (Some(rule), Some(status)) = (rule(event), event.status) else {
            return;
        };
        let credentialed = event.managed_credential_active != Some(false);
        let counts = match status {
            429 => true,
            401 | 403 => {
                credentialed && !self.take_excused(event, status) && !self.outside_repos(event)
            }
            200..=299 => credentialed,
            _ => false,
        };
        if !counts {
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
