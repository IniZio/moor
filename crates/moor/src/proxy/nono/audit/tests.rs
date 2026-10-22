//! Tests for draining nono's audit into log lines and Box events.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxId, BoxName};
use nono::undo::NetworkAuditEvent;
use serde_json::{json, Value};

use super::{apply, BoxSink, EgressSink, Refusal, Withheld};
use crate::proxy::ProxyLog;
use crate::store::Store;
use crate::testing::clock::FakeClock;

/// Sink recording each call as a string.
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    /// The calls so far.
    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl EgressSink for Recorder {
    fn allowed(&self, method: &str, route: &str, status: u16) {
        self.0
            .lock()
            .unwrap()
            .push(format!("allowed {method} {route} {status}"));
    }

    fn refused(&self, r: &Refusal<'_>) {
        let (host, rule, method, category) = (r.host, r.rule, r.method, r.category);
        self.0
            .lock()
            .unwrap()
            .push(format!("refused {host} {rule:?} {method:?} {category:?}"));
    }

    fn aborted(&self, _: &str, _: Option<&str>) {}

    fn credential_failed(&self, rule: &str) {
        self.0.lock().unwrap().push(format!("failed {rule}"));
    }

    fn withheld(&self, host: &str, rule: &str, method: &str, shape: Option<&str>) {
        self.0
            .lock()
            .unwrap()
            .push(format!("withheld {host} {rule} {method} {shape:?}"));
    }

    fn withheld_overflow(&self) {
        self.0.lock().unwrap().push("overflow".to_owned());
    }
}

/// An audit event from `extra` fields laid over a base of `decision` to `host`.
fn event(decision: &str, extra: &Value) -> NetworkAuditEvent {
    let mut base = json!({
        "timestamp_unix_ms": 0,
        "mode": "reverse",
        "decision": decision,
        "target": "api.example.com",
        "port": null,
        "method": null,
        "path": null,
        "status": null,
        "reason": null,
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    serde_json::from_value(base).unwrap()
}

/// Applies `events` and returns the sink's calls.
fn run(events: &[NetworkAuditEvent]) -> Vec<String> {
    let sink = Recorder::default();
    apply(events, &Withheld::default(), &sink);
    sink.calls()
}

#[test]
fn allow_becomes_a_log_line_named_by_rule() {
    let e = event(
        "allow",
        &json!({"route_id": "gh__0", "method": "GET", "status": 200, "path": "/secret/path"}),
    );
    assert_eq!(run(&[e]), ["allowed GET gh 200"]);
}

#[test]
fn allow_without_route_or_method_uses_host_and_dash() {
    assert_eq!(
        run(&[event("allow", &json!({}))]),
        ["allowed - api.example.com 0"]
    );
}

#[test]
fn capture_bookkeeping_and_policy_allows_are_not_logged() {
    let capture_ev = event("allow", &json!({"credential_capture_action": "captured"}));
    let policy = event("allow", &json!({"endpoint_policy_action": "allow"}));
    assert!(run(&[capture_ev, policy]).is_empty());
}

#[test]
fn host_denied_is_refused_without_rule() {
    let e = event("deny", &json!({"denial_category": "host_denied"}));
    assert_eq!(
        run(&[e]),
        ["refused api.example.com None None Some(\"host_denied\")"]
    );
}

#[test]
fn deny_on_a_route_names_the_rule() {
    let e = event(
        "deny",
        &json!({"route_id": "gh__1", "denial_category": "endpoint_policy"}),
    );
    assert_eq!(
        run(&[e]),
        ["refused api.example.com Some(\"gh\") None Some(\"endpoint_policy\")"]
    );
}

#[test]
fn authentication_failure_is_refused() {
    let e = event("deny", &json!({"denial_category": "authentication_failed"}));
    assert_eq!(
        run(&[e]),
        ["refused api.example.com None None Some(\"authentication_failed\")"]
    );
}

#[test]
fn unavailable_credential_is_a_credential_failure() {
    let e = event(
        "deny",
        &json!({
            "route_id": "gh__0",
            "denial_category": "managed_credential_unavailable",
            "credential_capture_name": "gh",
        }),
    );
    assert_eq!(run(&[e]), ["failed gh"]);
}

#[test]
fn credential_failure_falls_back_to_the_route_rule() {
    let e = event(
        "deny",
        &json!({"route_id": "gh__0", "denial_category": "managed_credential_unavailable"}),
    );
    assert_eq!(run(&[e]), ["failed gh"]);
}

/// The two denials nono emits for one request whose credential capture fails: the capture's,
/// then the request's.
fn failed_capture_pair() -> [NetworkAuditEvent; 2] {
    let category = "managed_credential_unavailable";
    [
        event(
            "deny",
            &json!({
                "route_id": "gh__0",
                "denial_category": category,
                "credential_capture_action": "failed",
                "credential_capture_name": "gh",
            }),
        ),
        event(
            "deny",
            &json!({"route_id": "gh__0", "denial_category": category}),
        ),
    ]
}

#[test]
fn one_request_whose_capture_fails_is_one_credential_failure() {
    assert_eq!(run(&failed_capture_pair()), ["failed gh"]);
    let [capture, request] = failed_capture_pair();
    let mut split = run(&[capture]);
    split.extend(run(&[request]));
    assert_eq!(split, ["failed gh"], "the pair split across two drains");
}

#[test]
fn approval_decisions_are_ignored() {
    let events = [
        event("approve_requested", &json!({})),
        event("approve_granted", &json!({})),
        event("approve_denied", &json!({})),
    ];
    assert!(run(&events).is_empty());
}

/// A store with one Box and a proxy log, both in `dir`.
fn box_sink_parts(dir: &Path) -> (Store, ProxyLog, BoxId) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock.clone()).unwrap();
    let new = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: None,
    };
    let box_id = store.apply(Decision::insert(new)).unwrap().box_id;
    let log = ProxyLog::open(dir, box_id, clock).unwrap();
    (store, log, box_id)
}

#[test]
fn box_sink_writes_events_and_a_path_free_log() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let events = [
        event(
            "allow",
            &json!({
                "route_id": "gh__0",
                "method": "POST",
                "status": 200,
                "path": "/repos/x/y?token=abc",
            }),
        ),
        event(
            "deny",
            &json!({"denial_category": "host_denied", "target": "evil.test"}),
        ),
        event(
            "deny",
            &json!({
                "route_id": "gh__0",
                "denial_category": "managed_credential_unavailable",
            }),
        ),
    ];
    apply(&events, &Withheld::default(), &sink);

    let rows = store.events(box_id, 100).unwrap();
    let of = |kind: &str| {
        rows.iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.payload.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        of("egress_refused"),
        [json!({"host": "evil.test", "rule": null, "method": null, "category": "host_denied"})]
    );
    assert_eq!(of("proxy_credential_failed"), [json!({"rule": "gh"})]);

    let text = std::fs::read_to_string(
        dir.path()
            .join("proxy")
            .join(format!("{}.log", box_id.get())),
    )
    .unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert!(lines.iter().any(|l| l.ends_with("POST gh 200")), "{text}");
    assert!(
        lines.iter().any(|l| l.ends_with("- evil.test 403")),
        "{text}"
    );
    assert!(!text.contains("repos") && !text.contains("token"), "{text}");
}

#[test]
fn box_sink_records_method_and_category_without_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let denied = event(
        "deny",
        &json!({
            "denial_category": "host_denied",
            "target": "evil.test",
            "method": "GET",
            "path": "/secret/path?k=v",
        }),
    );
    apply(
        &[denied, event("deny", &json!({"target": "bare.test"}))],
        &Withheld::default(),
        &sink,
    );

    let rows = store.events(box_id, 100).unwrap();
    let got: Vec<_> = rows.iter().map(|r| r.payload.clone()).collect();
    assert!(got.contains(&json!(
        {"host": "evil.test", "rule": null, "method": "GET", "category": "host_denied"}
    )));
    assert!(got.contains(&json!(
        {"host": "bare.test", "rule": null, "method": null, "category": null}
    )));
    let stored = serde_json::to_string(&got).unwrap();
    assert!(
        !stored.contains("secret") && !stored.contains("k=v"),
        "{stored}"
    );
}

#[test]
fn box_sink_swallows_store_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let missing = BoxId::new(box_id.get() + 100);
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id: missing,
    };
    sink.refused(&Refusal {
        host: "h.test",
        ..Refusal::default()
    });
    sink.credential_failed("gh");
}

/// A rule `gh` for `api.github.com:443`.
fn gh_rule() -> ResolvedRule {
    ResolvedRule {
        name: "gh".to_owned(),
        hosts: vec!["api.github.com:443".to_owned()],
        credential: "forge".to_owned(),
        inject: ResolvedInject::BasicAuth {
            user: "x".to_owned(),
        },
        endpoint_rules: Vec::new(),
        box_env: BTreeMap::new(),
    }
}

/// A passthrough response event for `method` `path` on `host`.
fn passthrough(host: &str, method: &str, path: &str) -> NetworkAuditEvent {
    event(
        "allow",
        &json!({
            "target": host,
            "method": method,
            "path": path,
            "status": 200,
            "managed_credential_active": false,
        }),
    )
}

/// Applies `events` against the `gh` rule and returns the sink's calls.
fn run_withheld(events: &[NetworkAuditEvent]) -> Vec<String> {
    let watch = Withheld::new(&[gh_rule()], Some(ForgeRepo::new("acme", "widgets")));
    let sink = Recorder::default();
    apply(events, &watch, &sink);
    sink.calls()
        .into_iter()
        .filter(|c| c.starts_with("withheld"))
        .collect()
}

#[test]
fn passthrough_on_a_credential_host_is_withheld_with_a_shape() {
    let got = run_withheld(&[passthrough(
        "api.github.com",
        "GET",
        "/repos/other/thing/issues/42?access=1",
    )]);
    assert_eq!(
        got,
        [r#"withheld api.github.com gh GET Some("GET /repos/other/thing/issues/*")"#]
    );
}

#[test]
fn repeated_shapes_are_withheld_once() {
    let got = run_withheld(&[
        passthrough("api.github.com", "GET", "/repos/other/thing/issues/1"),
        passthrough("api.github.com", "GET", "/repos/other/thing/issues/2"),
        passthrough("api.github.com", "POST", "/repos/other/thing/issues/2"),
    ]);
    assert_eq!(got.len(), 2, "{got:?}");
}

#[test]
fn non_credential_hosts_and_injected_requests_are_not_withheld() {
    let injected = event(
        "allow",
        &json!({
            "target": "api.github.com",
            "method": "GET",
            "status": 200,
            "managed_credential_active": true,
        }),
    );
    let got = run_withheld(&[passthrough("tunnel.test", "GET", "/x"), injected]);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn withheld_event_and_log_never_hold_the_raw_path() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let watch = Withheld::new(&[gh_rule()], Some(ForgeRepo::new("acme", "widgets")));
    apply(
        &[passthrough(
            "api.github.com",
            "GET",
            "/repos/other/thing/issues/42?token=hunter2",
        )],
        &watch,
        &sink,
    );

    let rows = store.events(box_id, 100).unwrap();
    let withheld: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "credential_withheld")
        .map(|r| r.payload.clone())
        .collect();
    assert_eq!(
        withheld,
        [json!({
            "host": "api.github.com",
            "rule": "gh",
            "method": "GET",
            "shape": "GET /repos/other/thing/issues/*",
        })]
    );
    let stored =
        serde_json::to_string(&rows.iter().map(|r| r.payload.clone()).collect::<Vec<_>>()).unwrap();
    let text = std::fs::read_to_string(
        dir.path()
            .join("proxy")
            .join(format!("{}.log", box_id.get())),
    )
    .unwrap();
    for seen in [stored, text] {
        assert!(!seen.contains("hunter2") && !seen.contains("42"), "{seen}");
    }
}

#[test]
fn withheld_reports_stop_at_the_cap_with_one_overflow_line() {
    let events: Vec<_> = (0..300)
        .map(|i| passthrough("api.github.com", "GET", &format!("/user/thing{i}")))
        .collect();
    let watch = Withheld::new(&[gh_rule()], Some(ForgeRepo::new("acme", "widgets")));
    let sink = Recorder::default();
    apply(&events, &watch, &sink);
    let calls = sink.calls();
    let reported = calls.iter().filter(|c| c.starts_with("withheld ")).count();
    let overflow = calls.iter().filter(|c| *c == "overflow").count();
    assert_eq!((reported, overflow), (256, 1));
}

#[test]
fn withheld_skips_a_method_that_is_not_upper_case_letters() {
    let got = run_withheld(&[
        passthrough("api.github.com", "get", "/user"),
        passthrough("api.github.com", "G;T", "/user"),
        event(
            "allow",
            &json!({"target": "api.github.com", "status": 200, "managed_credential_active": false}),
        ),
    ]);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn withheld_needs_a_response_status() {
    let pending = event(
        "allow",
        &json!({
            "target": "api.github.com",
            "method": "GET",
            "path": "/user",
            "managed_credential_active": false,
        }),
    );
    assert!(run_withheld(&[pending]).is_empty());
}

#[test]
fn withheld_matches_the_host_ignoring_case_and_port() {
    let got = run_withheld(&[passthrough("API.GitHub.com:443", "GET", "/user")]);
    assert_eq!(got, [r#"withheld API.GitHub.com gh GET Some("GET /user")"#]);
}

mod handshake;
mod limits;
