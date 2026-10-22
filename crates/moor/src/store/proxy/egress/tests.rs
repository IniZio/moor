//! Tests for recording proxy audit events.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};
use serde_json::json;

use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;

/// A store in a temp dir with one box.
fn setup(dir: &std::path::Path) -> (Store, BoxId) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    (store, id)
}

#[test]
fn egress_refused_is_appended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::EgressRefused {
        host: "evil.example".into(),
        rule: Some("gh".into()),
        method: None,
        category: None,
        reason: None,
    };
    store.record_proxy_event(id, &e).unwrap();
    let rows = store.events(id, 100).unwrap();
    let row = rows.iter().find(|r| r.kind == "egress_refused").unwrap();
    assert_eq!(
        row.payload,
        json!({"host": "evil.example", "rule": "gh", "method": null, "category": null})
    );
    assert_eq!(row.ts, Ts::from_millis(1_000));
}

#[test]
fn credential_withheld_is_appended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::CredentialWithheld {
        host: "api.github.com".into(),
        rule: "gh".into(),
        method: "GET".into(),
        shape: Some("GET /repos/{repo}/pulls/*".into()),
    };
    store.record_proxy_event(id, &e).unwrap();
    let rows = store.events(id, 100).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "credential_withheld")
        .unwrap();
    assert_eq!(
        row.payload,
        json!({"host": "api.github.com", "rule": "gh", "method": "GET", "shape": "GET /repos/{repo}/pulls/*"})
    );
}

#[test]
fn credential_failure_is_appended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::ProxyCredentialFailed { rule: "gh".into() };
    store.record_proxy_event(id, &e).unwrap();
    let rows = store.events(id, 100).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "proxy_credential_failed")
        .unwrap();
    assert_eq!(row.payload, json!({"rule": "gh"}));
}

#[test]
fn other_events_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let before = store.events(id, 100).unwrap().len();
    let e = Event::ProxyStarted {
        box_port: 1,
        pid: 3,
        handle: "h".into(),
    };
    let err = store.record_proxy_event(id, &e).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert_eq!(store.events(id, 100).unwrap().len(), before);
}

#[test]
fn unknown_box_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let e = Event::ProxyCredentialFailed { rule: "gh".into() };
    let err = store
        .record_proxy_event(BoxId::new(id.get() + 100), &e)
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");
}

#[test]
fn limit_events_are_accepted_and_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let events = [
        Event::AgentRateLimited {
            rule: "claude".into(),
            status: 429,
            resets_at: None,
        },
        Event::ProxyCredentialRejected {
            rule: "claude".into(),
            status: 401,
        },
        Event::ProxyCredentialRecovered {
            rule: "claude".into(),
        },
    ];
    for e in &events {
        store.record_proxy_event(id, e).unwrap();
    }
    let rows = store.events(id, 100).unwrap();
    for kind in [
        "agent_rate_limited",
        "proxy_credential_rejected",
        "proxy_credential_recovered",
    ] {
        assert!(rows.iter().any(|r| r.kind == kind), "{kind} missing");
    }
}

#[test]
fn limit_events_are_only_the_three_kinds_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let other = Event::ProxyCredentialFailed { rule: "gh".into() };
    store.record_proxy_event(id, &other).unwrap();
    store
        .record_proxy_event(
            id,
            &Event::AgentRateLimited {
                rule: "claude".into(),
                status: 429,
                resets_at: None,
            },
        )
        .unwrap();
    store.record_proxy_event(id, &other).unwrap();
    store
        .record_proxy_event(
            id,
            &Event::ProxyCredentialRecovered {
                rule: "claude".into(),
            },
        )
        .unwrap();
    let got = store.proxy_limit_events(id).unwrap();
    let kinds: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["agent_rate_limited", "proxy_credential_recovered"]);
    assert_eq!(got.last().map(|(_, p)| p), Some(&json!({"rule": "claude"})));
}

#[test]
fn open_limits_empty_after_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let (store, id) = setup(dir.path());
    let limited = Event::AgentRateLimited {
        rule: "claude".into(),
        status: 429,
        resets_at: None,
    };
    store.record_proxy_event(id, &limited).unwrap();
    assert_eq!(store.open_limits(id).unwrap().len(), 1);
    let recovered = Event::ProxyCredentialRecovered {
        rule: "claude".into(),
    };
    store.record_proxy_event(id, &recovered).unwrap();
    assert!(store.open_limits(id).unwrap().is_empty());
}
