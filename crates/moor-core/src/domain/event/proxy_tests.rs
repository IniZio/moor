//! Tests for the proxy audit event payloads.

use super::*;

fn s(v: &str) -> String {
    v.to_owned()
}

/// The egress and proxy credential events serialize with their kind tag and payload.
#[test]
fn egress_events_serialize_with_kind_tag() {
    let cases = [
        (
            Event::EgressRefused {
                host: s("h"),
                rule: Some(s("r")),
                method: Some(s("GET")),
                category: Some(s("host_denied")),
            },
            "egress_refused",
            serde_json::json!({"host": "h", "rule": "r", "method": "GET", "category": "host_denied"}),
        ),
        (
            Event::CredentialWithheld {
                host: s("h"),
                rule: s("r"),
                method: s("GET"),
                shape: Some(s("GET /repos/{repo}/pulls/*")),
            },
            "credential_withheld",
            serde_json::json!({"host": "h", "rule": "r", "method": "GET", "shape": "GET /repos/{repo}/pulls/*"}),
        ),
        (
            Event::ProxyCredentialFailed { rule: s("r") },
            "proxy_credential_failed",
            serde_json::json!({"rule": "r"}),
        ),
    ];
    for (e, kind, payload) in cases {
        assert_eq!(e.kind(), kind);
        assert_eq!(e.payload(), payload);
        assert!(!e.is_asserted());
        let mut full = payload.as_object().cloned().unwrap();
        full.insert("kind".into(), kind.into());
        let full = serde_json::Value::Object(full);
        assert_eq!(serde_json::to_value(&e).unwrap(), full);
    }
}
