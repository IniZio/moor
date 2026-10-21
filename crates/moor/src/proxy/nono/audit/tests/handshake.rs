//! The reason recorded for a denial, by its category.

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
    let (capture, _) = counting_capture();
    let text = "tls handshake failed: tls handshake eof";
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
    apply(&events, &capture, &Withheld::default(), &sink);

    let rows = store.events(box_id, 100).unwrap();
    let payload = |host: &str| {
        rows.iter()
            .map(|r| &r.payload)
            .find(|p| p["host"] == host)
            .unwrap()
    };
    assert!(payload("a.test").get("reason").is_none());
    assert_eq!(payload("b.test")["reason"], "client_closed");
}
