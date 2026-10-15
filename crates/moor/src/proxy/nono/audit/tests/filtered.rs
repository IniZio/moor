//! Audited 401s for a rule whose evictions the GraphQL filter drives.

use super::*;

/// An audited 401 for route `gh__0`.
fn gh_401() -> NetworkAuditEvent {
    event(
        "allow",
        &json!({"route_id": "gh__0", "method": "POST", "status": 401}),
    )
}

#[test]
fn audited_401_keeps_the_credential_of_a_rule_the_filter_owns() {
    let (mut capture, count) = counting_capture();
    capture.defer_evictions(["gh".to_owned()]);
    fetch(&capture);
    apply(
        &[gh_401()],
        &capture,
        &Withheld::default(),
        &Recorder::default(),
    );
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn audited_401_still_evicts_a_rule_the_filter_does_not_own() {
    let (mut capture, count) = counting_capture();
    capture.defer_evictions(["other".to_owned()]);
    fetch(&capture);
    apply(
        &[gh_401()],
        &capture,
        &Withheld::default(),
        &Recorder::default(),
    );
    fetch(&capture);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
