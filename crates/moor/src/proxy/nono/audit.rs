//! Drains nono's network audit into per-Box proxy log lines and Box events.
//!
//! Only the method, the rule name (or host), the status and the denial category leave this module.
//! Headers and bodies in the audit are never read. The path is read only to
//! build a withheld-credential template; it is never stored or logged.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use moor_core::config::proxy::{request_shape, ResolvedRule};
use moor_core::domain::event::Event;
use moor_core::seam::forge::ForgeRepo;
use moor_core::BoxId;
use nono::undo::{NetworkAuditDecision, NetworkAuditDenialCategory, NetworkAuditEvent};
use serde_json::Value;

use self::limits::Limits;
use super::capture::MoorCapture;
use super::rule_of_route;
use crate::proxy::ProxyLog;
use crate::store::Store;

/// Where audit outcomes go.
pub(crate) trait EgressSink {
    /// Records one allowed request as a proxy log line.
    fn allowed(&self, method: &str, route: &str, status: u16);
    /// Records a refused egress attempt to `host`, with the rule, request
    /// method and denial category when known.
    fn refused(&self, refusal: &Refusal<'_>);
    /// Records that a client hung up on an intercepted handshake to `host` without an alert.
    fn aborted(&self, host: &str, rule: Option<&str>);
    /// Records that the credential for `rule` could not be obtained.
    fn credential_failed(&self, rule: &str);
    /// Records a request to a credential host that was forwarded without
    /// the credential, with its templated shape when one is sound.
    fn withheld(&self, host: &str, rule: &str, method: &str, shape: Option<&str>);
    /// Records, once, that further distinct withheld requests are not reported.
    fn withheld_overflow(&self);
    /// The status of the open limit on `rule` recorded earlier, if any.
    fn open_limit(&self, _rule: &str) -> Option<u16> {
        None
    }
    /// Records a rate limit, rejection or recovery of a credential rule.
    fn limit(&self, _event: &Event) {}
}

/// One refused egress attempt.
#[derive(Default)]
pub(crate) struct Refusal<'a> {
    /// The host the connection was for.
    pub(crate) host: &'a str,
    /// The rule the host belongs to, when known.
    pub(crate) rule: Option<&'a str>,
    /// The request method, when one was seen.
    pub(crate) method: Option<&'a str>,
    /// The Proxy's denial category.
    pub(crate) category: Option<&'a str>,
    /// What ended an intercepted TLS handshake.
    pub(crate) reason: Option<&'a str>,
}

/// Distinct withheld requests reported per Box before reporting stops.
const MAX_WITHHELD: usize = 256;

/// What has been reported for one Box.
#[derive(Default)]
struct Reported {
    /// `(rule, method, shape)` already reported.
    keys: BTreeSet<(String, String, Option<String>)>,
    /// Whether the overflow line has been written.
    overflowed: bool,
}

/// What the drain needs to spot requests forwarded without a credential.
#[derive(Default)]
pub(crate) struct Withheld {
    /// Rule name by lower-case bare host.
    hosts: BTreeMap<String, String>,
    /// The Box repo the shapes are templated against.
    repo: Option<ForgeRepo>,
    /// What has been reported for this Box.
    seen: Mutex<Reported>,
    /// Rate limits and rejections reported for this Box.
    limits: Limits,
}

impl Withheld {
    /// Watches every host the `rules` list; shapes are templated against `repo`.
    pub(crate) fn new(rules: &[ResolvedRule], repo: Option<ForgeRepo>) -> Self {
        let mut hosts = BTreeMap::new();
        for rule in rules {
            for host in &rule.hosts {
                hosts
                    .entry(bare_host(host).to_ascii_lowercase())
                    .or_insert_with(|| rule.name.clone());
            }
        }
        Self {
            hosts,
            repo,
            seen: Mutex::default(),
            limits: Limits::default(),
        }
    }

    /// Reports `event` to `sink` once per `(rule, method, shape)` when it is a
    /// credential-less passthrough to a credential host.
    fn report(&self, event: &NetworkAuditEvent, sink: &dyn EgressSink) {
        if event.managed_credential_active != Some(false)
            || event.status.is_none()
            || event.credential_capture_action.is_some()
        {
            return;
        }
        let host = bare_host(&event.target);
        let Some(host_rule) = self.hosts.get(&host.to_ascii_lowercase()) else {
            return;
        };
        let rule = rule(event).unwrap_or(host_rule);
        let Some(method) = event.method.as_deref() else {
            return;
        };
        self.record(
            host,
            rule,
            method,
            event.path.as_deref().unwrap_or("/"),
            sink,
        );
    }

    /// Reports a request to credential `host` that was forwarded without the
    /// credential, as the GraphQL filter saw it.
    pub(crate) fn note(&self, host: &str, method: &str, path: &str, sink: &dyn EgressSink) {
        if let Some(rule) = self.hosts.get(&bare_host(host).to_ascii_lowercase()) {
            self.record(host, rule, method, path, sink);
        }
    }

    /// Notes that the next audited `status` from `host` says nothing about the
    /// credential, so it is not a rejected credential.
    pub(crate) fn excuse(&self, host: &str, status: u16) {
        self.limits.excuse(host, status);
    }

    /// Reports one withheld request once per `(rule, method, shape)`.
    fn record(&self, host: &str, rule: &str, method: &str, path: &str, sink: &dyn EgressSink) {
        if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
            return;
        }
        let shape = self
            .repo
            .as_ref()
            .and_then(|repo| request_shape(method, path, (&repo.owner, &repo.name)));
        let key = (rule.to_owned(), method.to_owned(), shape.clone());
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.keys.contains(&key) {
            return;
        }
        if seen.keys.len() >= MAX_WITHHELD {
            if !seen.overflowed {
                seen.overflowed = true;
                sink.withheld_overflow();
            }
            return;
        }
        seen.keys.insert(key);
        drop(seen);
        sink.withheld(host, rule, method, shape.as_deref());
    }
}

/// `host` without a trailing `:port`.
fn bare_host(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

/// Production sink: proxy log lines and store events for one Box.
pub(crate) struct BoxSink<'a> {
    /// The Box's request log.
    pub(crate) log: &'a ProxyLog,
    /// Store receiving the Box's events.
    pub(crate) store: &'a Store,
    /// The Box the proxy serves.
    pub(crate) box_id: BoxId,
}

impl EgressSink for BoxSink<'_> {
    fn allowed(&self, method: &str, route: &str, status: u16) {
        self.log.record(method, route, status);
    }

    fn refused(&self, r: &Refusal<'_>) {
        self.log
            .record(r.method.unwrap_or("-"), r.rule.unwrap_or(r.host), 403);
        let event = Event::EgressRefused {
            host: r.host.to_owned(),
            rule: r.rule.map(str::to_owned),
            method: r.method.map(str::to_owned),
            category: r.category.map(str::to_owned),
            reason: r.reason.map(str::to_owned),
        };
        // Draining must not fail; a lost event is not worth stopping the proxy.
        let _ = self.store.record_proxy_event(self.box_id, &event);
    }

    fn aborted(&self, host: &str, rule: Option<&str>) {
        let event = Event::EgressAborted {
            host: host.to_owned(),
            rule: rule.map(str::to_owned),
        };
        let _ = self.store.record_proxy_event(self.box_id, &event);
    }

    fn credential_failed(&self, rule: &str) {
        let event = Event::ProxyCredentialFailed {
            rule: rule.to_owned(),
        };
        let _ = self.store.record_proxy_event(self.box_id, &event);
    }

    fn withheld(&self, host: &str, rule: &str, method: &str, shape: Option<&str>) {
        let event = Event::CredentialWithheld {
            host: host.to_owned(),
            rule: rule.to_owned(),
            method: method.to_owned(),
            shape: shape.map(str::to_owned),
        };
        let _ = self.store.record_proxy_event(self.box_id, &event);
    }

    fn withheld_overflow(&self) {
        self.log.record("-", "withheld-overflow", 0);
    }

    fn open_limit(&self, rule: &str) -> Option<u16> {
        let limits = self.store.open_limits(self.box_id).ok()?;
        limits.iter().find(|l| l.rule == rule).map(|l| l.status)
    }

    fn limit(&self, event: &Event) {
        let _ = self.store.record_proxy_event(self.box_id, event);
    }
}

/// Applies one drained batch of audit events.
///
/// An allowed request becomes a log line, and an upstream 401 also evicts the
/// rule's cached credential unless the request was a passthrough. A denial caused by an unavailable credential
/// becomes a credential failure, and an intercepted handshake the client hung up on without an
/// alert becomes an aborted egress; any other denial becomes a refused egress.
/// Approval-flow decisions are ignored. A request forwarded without a
/// credential to a credential host is also reported through `withheld`. An
/// excused answer noted before this drain is matched in it or the next one.
pub(crate) fn apply(
    events: &[NetworkAuditEvent],
    capture: &MoorCapture,
    withheld: &Withheld,
    sink: &dyn EgressSink,
) {
    for event in events {
        match event.decision {
            NetworkAuditDecision::Allow => {
                allow(event, capture, sink);
                withheld.limits.observe(event, sink);
                withheld.report(event, sink);
            }
            NetworkAuditDecision::Deny => deny(event, sink),
            _ => {}
        }
    }
    withheld.limits.end_drain();
}

/// The rule behind an event's route, if any.
fn rule(event: &NetworkAuditEvent) -> Option<&str> {
    event.route_id.as_deref().and_then(rule_of_route)
}

/// Handles an allowed event.
fn allow(event: &NetworkAuditEvent, capture: &MoorCapture, sink: &dyn EgressSink) {
    // Credential-capture bookkeeping and pre-request policy decisions carry no
    // response; the response event that follows is the request's log line.
    if event.credential_capture_action.is_some()
        || (event.status.is_none() && event.endpoint_policy_action.is_some())
    {
        return;
    }
    let rule = rule(event);
    sink.allowed(
        event.method.as_deref().unwrap_or("-"),
        rule.unwrap_or(&event.target),
        event.status.unwrap_or(0),
    );
    // A passthrough carries no managed credential, so its 401 says nothing about the cache.
    if event.status == Some(401) && event.managed_credential_active != Some(false) {
        if let Some(rule) = rule {
            capture.evict_audited(rule);
        }
    }
}

/// Handles a denied event.
fn deny(event: &NetworkAuditEvent, sink: &dyn EgressSink) {
    let failed = event.denial_category
        == Some(NetworkAuditDenialCategory::ManagedCredentialUnavailable)
        || matches!(
            event.credential_capture_action.as_deref(),
            Some("denied" | "failed")
        );
    if failed {
        let name = event
            .credential_capture_name
            .as_deref()
            .or_else(|| rule(event));
        if let Some(name) = name {
            sink.credential_failed(name);
            return;
        }
    }
    let reason = (event.denial_category
        == Some(NetworkAuditDenialCategory::InterceptHandshakeFailed))
    .then(|| handshake::reason(event.reason.as_deref().unwrap_or_default()));
    if reason.as_deref() == Some(handshake::CLIENT_CLOSED) {
        sink.aborted(&event.target, rule(event));
        return;
    }
    sink.refused(&Refusal {
        host: &event.target,
        rule: rule(event),
        method: event.method.as_deref(),
        category: event
            .denial_category
            .as_ref()
            .and_then(category_name)
            .as_deref(),
        reason: reason.as_deref(),
    });
}

/// The `snake_case` name nono gives a denial category.
fn category_name(category: &NetworkAuditDenialCategory) -> Option<String> {
    match serde_json::to_value(category).ok()? {
        Value::String(name) => Some(name),
        _ => None,
    }
}

mod handshake;
mod limits;

#[cfg(test)]
mod tests;
