//! Tests for draining nono's audit into log lines and Task events.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxId, BoxName, Secret};
use nono::undo::NetworkAuditEvent;
use nono_proxy::capture::{CredentialCaptureBackend as _, CredentialCaptureRequest};
use serde_json::{json, Value};

use super::{apply, EgressSink, TaskSink, Withheld};
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
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

    fn refused(
        &self,
        host: &str,
        rule: Option<&str>,
        method: Option<&str>,
        category: Option<&str>,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(format!("refused {host} {rule:?} {method:?} {category:?}"));
    }

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

/// A capture whose single rule `gh` counts its fetches.
fn counting_capture() -> (MoorCapture, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let source = RuleSource {
        config_key: "credentials.GH".into(),
        fetch: Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Secret::new(b"tok".to_vec()))
        }),
        basic_user: None,
    };
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let sources = BTreeMap::from([("gh".to_owned(), source)]);
    (MoorCapture::new(sources, clock), count)
}

/// Asks the capture for rule `gh`'s credential.
fn fetch(capture: &MoorCapture) {
    let request = CredentialCaptureRequest {
        credential_name: "gh".into(),
        route_id: "gh__0".into(),
        request_host: "api.example.com".into(),
        request_path: "/".into(),
        request_method: "GET".into(),
        session_id: "s".into(),
        cache_scope: "s".into(),
    };
    assert!(capture.capture(request).is_ok());
}

/// Applies `events` with a fresh capture and returns the sink's calls.
fn run(events: &[NetworkAuditEvent]) -> Vec<String> {
    let (capture, _) = counting_capture();
    let sink = Recorder::default();
    apply(events, &capture, &Withheld::default(), &sink);
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
fn allow_401_evicts_the_cached_credential() {
    let (capture, count) = counting_capture();
    fetch(&capture);
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let e = event(
        "allow",
        &json!({"route_id": "gh__0", "method": "GET", "status": 401}),
    );
    let sink = Recorder::default();
    apply(&[e], &capture, &Withheld::default(), &sink);
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(sink.calls(), ["allowed GET gh 401"]);
}

#[test]
fn passthrough_401_does_not_evict_the_cached_credential() {
    let (capture, count) = counting_capture();
    fetch(&capture);
    let e = event(
        "allow",
        &json!({
            "route_id": "gh__0",
            "method": "GET",
            "status": 401,
            "managed_credential_active": false,
        }),
    );
    apply(&[e], &capture, &Withheld::default(), &Recorder::default());
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 1);

    let injected = event(
        "allow",
        &json!({
            "route_id": "gh__0",
            "method": "GET",
            "status": 401,
            "managed_credential_active": true,
        }),
    );
    apply(
        &[injected],
        &capture,
        &Withheld::default(),
        &Recorder::default(),
    );
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn allow_401_without_a_rule_evicts_nothing() {
    let (capture, count) = counting_capture();
    fetch(&capture);
    let sink = Recorder::default();
    apply(
        &[event("allow", &json!({"status": 401}))],
        &capture,
        &Withheld::default(),
        &sink,
    );
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 1);
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

#[test]
fn denied_capture_action_is_a_credential_failure() {
    let e = event(
        "deny",
        &json!({"credential_capture_action": "denied", "credential_capture_name": "gh"}),
    );
    assert_eq!(run(&[e]), ["failed gh"]);
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

/// A store with one task and a proxy log, both in `dir`.
fn task_sink_parts(dir: &Path) -> (Store, ProxyLog, BoxId) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock.clone()).unwrap();
    let new = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: "r".into(),
    };
    let task = store.apply(Decision::insert(new)).unwrap().task;
    let log = ProxyLog::open(dir, task, clock).unwrap();
    (store, log, task)
}

#[test]
fn task_sink_writes_events_and_a_path_free_log() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, task) = task_sink_parts(dir.path());
    let sink = TaskSink {
        log: &log,
        store: &store,
        task,
    };
    let (capture, _) = counting_capture();
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
    apply(&events, &capture, &Withheld::default(), &sink);

    let rows = store.events(task, 100).unwrap();
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

    let text =
        std::fs::read_to_string(dir.path().join("proxy").join(format!("{}.log", task.get())))
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
fn task_sink_records_method_and_category_without_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, task) = task_sink_parts(dir.path());
    let sink = TaskSink {
        log: &log,
        store: &store,
        task,
    };
    let (capture, _) = counting_capture();
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
        &capture,
        &Withheld::default(),
        &sink,
    );

    let rows = store.events(task, 100).unwrap();
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
fn task_sink_swallows_store_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, task) = task_sink_parts(dir.path());
    let missing = BoxId::new(task.get() + 100);
    let sink = TaskSink {
        log: &log,
        store: &store,
        task: missing,
    };
    sink.refused("h.test", None, None, None);
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
    let (capture, _) = counting_capture();
    let watch = Withheld::new(&[gh_rule()], Some(ForgeRepo::new("acme", "widgets")));
    let sink = Recorder::default();
    apply(events, &capture, &watch, &sink);
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
    let (store, log, task) = task_sink_parts(dir.path());
    let sink = TaskSink {
        log: &log,
        store: &store,
        task,
    };
    let (capture, _) = counting_capture();
    let watch = Withheld::new(&[gh_rule()], Some(ForgeRepo::new("acme", "widgets")));
    apply(
        &[passthrough(
            "api.github.com",
            "GET",
            "/repos/other/thing/issues/42?token=hunter2",
        )],
        &capture,
        &watch,
        &sink,
    );

    let rows = store.events(task, 100).unwrap();
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
    let text =
        std::fs::read_to_string(dir.path().join("proxy").join(format!("{}.log", task.get())))
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
    let (capture, _) = counting_capture();
    let watch = Withheld::new(&[gh_rule()], Some(ForgeRepo::new("acme", "widgets")));
    let sink = Recorder::default();
    apply(&events, &capture, &watch, &sink);
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

mod filtered;
mod limits;
