//! Rate limits, rejections and recoveries reported from audited responses.

use moor_core::domain::event::Event;

use super::*;

/// Sink recording limit events and answering `open_limit` from a fixed status.
#[derive(Default)]
struct LimitSink {
    /// Events recorded through `limit`.
    events: Mutex<Vec<Event>>,
    /// The open limit status reported for every rule.
    open: Option<u16>,
}

impl EgressSink for LimitSink {
    fn allowed(&self, _: &str, _: &str, _: u16) {}
    fn refused(&self, _: &str, _: Option<&str>, _: Option<&str>, _: Option<&str>) {}
    fn credential_failed(&self, _: &str) {}
    fn withheld(&self, _: &str, _: &str, _: &str, _: Option<&str>) {}
    fn withheld_overflow(&self) {}

    fn open_limit(&self, _: &str) -> Option<u16> {
        self.open
    }

    fn limit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// An allowed response with `status` on rule `claude`.
fn response(status: u16) -> NetworkAuditEvent {
    event(
        "allow",
        &json!({"route_id": "claude__0", "method": "POST", "status": status}),
    )
}

/// Applies `events` in one drain and returns what the sink recorded.
fn run_limits(sink: &LimitSink, events: &[NetworkAuditEvent], watch: &Withheld) -> Vec<Event> {
    let (capture, _) = counting_capture();
    apply(events, &capture, watch, sink);
    sink.events.lock().unwrap().clone()
}

/// The rate limit event for `claude`.
fn limited() -> Event {
    Event::AgentRateLimited {
        rule: "claude".to_owned(),
        status: 429,
        resets_at: None,
    }
}

/// The recovery event for `claude`.
fn recovered() -> Event {
    Event::ProxyCredentialRecovered {
        rule: "claude".to_owned(),
    }
}

#[test]
fn a_429_records_a_rate_limit() {
    let got = run_limits(
        &LimitSink::default(),
        &[response(429)],
        &Withheld::default(),
    );
    assert_eq!(got, [limited()]);
}

#[test]
fn repeated_429s_record_one_event() {
    let events = [response(429), response(429), response(429)];
    let got = run_limits(&LimitSink::default(), &events, &Withheld::default());
    assert_eq!(got, [limited()]);
}

#[test]
fn a_2xx_after_a_limit_records_recovery_once_and_a_later_429_records_again() {
    let events = [response(429), response(200), response(200), response(429)];
    let got = run_limits(&LimitSink::default(), &events, &Withheld::default());
    assert_eq!(got, [limited(), recovered(), limited()]);
}

#[test]
fn a_2xx_with_no_open_limit_records_nothing() {
    let got = run_limits(
        &LimitSink::default(),
        &[response(200)],
        &Withheld::default(),
    );
    assert!(got.is_empty());
}

#[test]
fn an_allow_with_no_status_leaves_an_open_limit_open() {
    let failed_body = event("allow", &json!({"route_id": "claude__0", "method": "POST"}));
    let events = [response(429), failed_body, response(429)];
    let got = run_limits(&LimitSink::default(), &events, &Withheld::default());
    assert_eq!(got, [limited()]);
}

#[test]
fn a_401_records_a_rejection() {
    let got = run_limits(
        &LimitSink::default(),
        &[response(401)],
        &Withheld::default(),
    );
    assert_eq!(
        got,
        [Event::ProxyCredentialRejected {
            rule: "claude".to_owned(),
            status: 401,
        }]
    );
}

#[test]
fn a_passthrough_401_records_nothing() {
    let passthrough = event(
        "allow",
        &json!({
            "route_id": "claude__0",
            "method": "POST",
            "status": 401,
            "managed_credential_active": false,
        }),
    );
    let got = run_limits(&LimitSink::default(), &[passthrough], &Withheld::default());
    assert!(got.is_empty());
}

#[test]
fn a_429_then_a_401_record_both() {
    let got = run_limits(
        &LimitSink::default(),
        &[response(429), response(401)],
        &Withheld::default(),
    );
    assert_eq!(
        got,
        [
            limited(),
            Event::ProxyCredentialRejected {
                rule: "claude".to_owned(),
                status: 401,
            },
        ]
    );
}

/// The rejection event for `claude` with `status`.
fn rejected(status: u16) -> Event {
    Event::ProxyCredentialRejected {
        rule: "claude".to_owned(),
        status,
    }
}

#[test]
fn an_anonymous_note_no_audit_consumes_stops_hiding_rejections_after_one_more_drain() {
    let (sink, watch) = (LimitSink::default(), Withheld::default());
    watch.excuse("api.example.com", 403);
    assert!(run_limits(&sink, &[], &watch).is_empty());
    assert!(run_limits(&sink, &[], &watch).is_empty());
    assert_eq!(run_limits(&sink, &[response(403)], &watch), [rejected(403)]);
}

#[test]
fn an_anonymous_note_hides_its_audit_in_the_next_drain() {
    let (sink, watch) = (LimitSink::default(), Withheld::default());
    watch.excuse("api.example.com", 403);
    assert!(run_limits(&sink, &[], &watch).is_empty());
    assert!(run_limits(&sink, &[response(403)], &watch).is_empty());
    assert_eq!(run_limits(&sink, &[response(403)], &watch), [rejected(403)]);
}

#[test]
fn an_open_limit_from_the_store_suppresses_a_repeat_and_still_recovers() {
    let sink = LimitSink {
        open: Some(429),
        ..LimitSink::default()
    };
    let got = run_limits(&sink, &[response(429), response(200)], &Withheld::default());
    assert_eq!(got, [recovered()]);
}
