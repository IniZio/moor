//! Drains nono's network audit into per-Task proxy log lines and Task events.
//!
//! Only the method, the rule name (or host) and the status leave this module.
//! Headers, bodies, paths and query strings in the audit are never read.

use moor_core::domain::event::Event;
use moor_core::TaskId;
use nono::undo::{NetworkAuditDecision, NetworkAuditDenialCategory, NetworkAuditEvent};

use super::capture::MoorCapture;
use super::rule_of_route;
use crate::proxy::ProxyLog;
use crate::store::Store;

/// Where audit outcomes go.
pub(crate) trait EgressSink {
    /// Records one allowed request as a proxy log line.
    fn allowed(&self, method: &str, route: &str, status: u16);
    /// Records a refused egress attempt to `host`, with the rule when known.
    fn refused(&self, host: &str, rule: Option<&str>);
    /// Records that the credential for `rule` could not be obtained.
    fn credential_failed(&self, rule: &str);
}

/// Production sink: proxy log lines and store events for one Task.
pub(crate) struct TaskSink<'a> {
    /// The Task's request log.
    pub(crate) log: &'a ProxyLog,
    /// Store receiving the Task's events.
    pub(crate) store: &'a Store,
    /// The Task the proxy serves.
    pub(crate) task: TaskId,
}

impl EgressSink for TaskSink<'_> {
    fn allowed(&self, method: &str, route: &str, status: u16) {
        self.log.record(method, route, status);
    }

    fn refused(&self, host: &str, rule: Option<&str>) {
        let event = Event::EgressRefused {
            host: host.to_owned(),
            rule: rule.map(str::to_owned),
        };
        // Draining must not fail; a lost event is not worth stopping the proxy.
        let _ = self.store.record_proxy_event(self.task, &event);
    }

    fn credential_failed(&self, rule: &str) {
        let event = Event::ProxyCredentialFailed {
            rule: rule.to_owned(),
        };
        let _ = self.store.record_proxy_event(self.task, &event);
    }
}

/// Applies one drained batch of audit events.
///
/// An allowed request becomes a log line, and an upstream 401 also evicts the
/// rule's cached credential. A denial caused by an unavailable credential
/// becomes a credential failure; any other denial becomes a refused egress.
/// Approval-flow decisions are ignored.
pub(crate) fn apply(events: &[NetworkAuditEvent], capture: &MoorCapture, sink: &dyn EgressSink) {
    for event in events {
        match event.decision {
            NetworkAuditDecision::Allow => allow(event, capture, sink),
            NetworkAuditDecision::Deny => deny(event, sink),
            _ => {}
        }
    }
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
    if event.status == Some(401) {
        if let Some(rule) = rule {
            capture.evict(rule);
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
    sink.refused(&event.target, rule(event));
}

#[cfg(test)]
mod tests;
