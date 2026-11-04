//! What a failed intercepted handshake records, by how it ended.

use super::*;

#[test]
fn only_a_failed_handshake_denial_records_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let ledger = box_ledger(&store, box_id);
    let sink = BoxSink {
        log: &log,
        ledger: &ledger,
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
    let ledger = box_ledger(&store, box_id);
    let sink = BoxSink {
        log: &log,
        ledger: &ledger,
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
    let ledger = box_ledger(&store, box_id);
    let sink = BoxSink {
        log: &log,
        ledger: &ledger,
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

#[test]
fn a_connect_failure_records_a_fixed_word_not_nonos_text() {
    let dir = tempfile::tempdir().unwrap();
    let (store, log, box_id) = box_sink_parts(dir.path());
    let ledger = box_ledger(&store, box_id);
    let sink = BoxSink {
        log: &log,
        ledger: &ledger,
    };
    let failed = |host: &str, reason: &str| {
        event(
            "deny",
            &json!({"denial_category": "upstream_connect_failed", "target": host, "reason": reason}),
        )
    };
    let events = [
        failed("a.test", "Connection refused (os error 111)"),
        failed("b.test", "DNS resolution returned no addresses"),
        failed("c.test", "connection timed out"),
        failed("d.test", "Connection reset by peer (os error 104)"),
        failed("e.test", "TLS handshake failed: invalid peer certificate"),
        failed("f.test", "token=hunter2 at 10.0.0.7"),
    ];
    apply(&events, &Withheld::default(), &sink);

    let rows = store.events(box_id, 100).unwrap();
    let reason = |host: &str| {
        rows.iter()
            .map(|r| &r.payload)
            .find(|p| p["host"] == host)
            .unwrap()["reason"]
            .clone()
    };
    for (host, want) in [
        ("a.test", "refused"),
        ("b.test", "dns"),
        ("c.test", "timeout"),
        ("d.test", "reset"),
        ("e.test", "tls"),
        ("f.test", "other"),
    ] {
        assert_eq!(reason(host), want, "{host}");
    }
}
