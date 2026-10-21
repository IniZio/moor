//! What a failed intercepted handshake records, by how it ended.

use super::*;

#[test]
fn only_a_failed_handshake_denial_records_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let text = "tls handshake failed: received fatal alert: UnknownCA";
    let events = [
        event(
            "deny",
            &json!({"denial_category": "host_denied", "target": "a.test", "reason": text}),
        ),
        event(
            "deny",
            &json!({"denial_category": "intercept_handshake_failed", "target": "b.test", "reason": text}),
        ),
    ];
    apply(&events, &Withheld::default(), &sink);

    let rows = store.events(box_id, 100).unwrap();
    let payload = |host: &str| {
        rows.iter()
            .map(|r| &r.payload)
            .find(|p| p["host"] == host)
            .unwrap()
    };
    assert!(payload("a.test").get("reason").is_none());
    assert_eq!(payload("b.test")["reason"], "unknown_ca_alert");
}

#[test]
fn a_handshake_the_client_hangs_up_on_is_aborted_and_an_alert_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let failed = |host: &str, reason: &str| {
        event(
            "deny",
            &json!({
                "denial_category": "intercept_handshake_failed",
                "route_id": "claude__0",
                "target": host,
                "reason": reason,
            }),
        )
    };
    let events = [
        failed("eof.test", "tls handshake failed: tls handshake eof"),
        failed(
            "ca.test",
            "tls handshake failed: received fatal alert: UnknownCA",
        ),
    ];
    apply(&events, &Withheld::default(), &sink);

    let rows: Vec<_> = store
        .events(box_id, 100)
        .unwrap()
        .into_iter()
        .filter(|r| r.kind.starts_with("egress_"))
        .map(|r| (r.kind, r.payload))
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.contains(&(
            "egress_aborted".to_owned(),
            json!({"host": "eof.test", "rule": "claude"})
        )),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(k, p)| k == "egress_refused" && p["host"] == "ca.test"),
        "{rows:?}"
    );
    let text = std::fs::read_to_string(
        dir.path()
            .join("proxy")
            .join(format!("{}.log", box_id.get())),
    )
    .unwrap();
    assert!(!text.contains("eof.test"), "{text}");
    assert!(text.contains("- claude 403"), "{text}");
}

#[test]
fn another_alert_or_an_unknown_ending_is_still_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let failed = |host: &str, reason: &str| {
        event(
            "deny",
            &json!({"denial_category": "intercept_handshake_failed", "target": host, "reason": reason}),
        )
    };
    let events = [
        failed(
            "alert.test",
            "tls handshake failed: received fatal alert: BadCertificate",
        ),
        failed("other.test", "tls handshake failed: something new"),
    ];
    apply(&events, &Withheld::default(), &sink);

    let rows: Vec<_> = store
        .events(box_id, 100)
        .unwrap()
        .into_iter()
        .filter(|r| r.kind.starts_with("egress_"))
        .map(|r| {
            let field = |k: &str| r.payload.get(k).cloned().unwrap_or_default();
            (r.kind.clone(), field("host"), field("reason"))
        })
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    for (host, reason) in [
        ("alert.test", "other_alert:BadCertificate"),
        ("other.test", "other"),
    ] {
        assert!(
            rows.contains(&("egress_refused".to_owned(), json!(host), json!(reason))),
            "{rows:?}"
        );
    }
}
