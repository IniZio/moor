//! The per-provider lines from the controller's `provider-ready`.

use serde_json::json;

use super::*;

fn rig(ready: Result<Value, ClientError>) -> Rig {
    Rig::new().with_controller(Box::new(move |r| match (r.cmd(), &ready) {
        ("provider-ready", Ok(v)) => answered("provider-ready", v.clone()),
        ("provider-ready", Err(_)) => Err(ClientError::Protocol("down".to_owned())),
        (cmd, _) => Err(ClientError::Usage(format!("unscripted {cmd}"))),
    }))
}

fn lines(report: &[Check]) -> Vec<&Check> {
    report.iter().filter(|c| c.name == "crabbox").collect()
}

#[test]
fn a_failing_and_a_passing_provider_each_get_a_line() {
    let r = rig(Ok(json!({"providers": [
        {"provider": "aws", "ok": false, "failing": [
            {"check": "provider", "status": "failed", "hint": "set AWS_PROFILE"},
            {"check": "ssh", "status": "failed"}]},
        {"provider": "static", "ok": true, "failing": []}
    ]})));
    let report = r.with(provider::controller);
    let found = lines(&report);
    let [aws, st] = found.as_slice() else {
        return assert_eq!(found.len(), 2, "{report:?}");
    };
    assert_eq!(aws.status, Status::Fail);
    assert_eq!(aws.detail, "aws: provider (set AWS_PROFILE), ssh");
    assert_eq!(
        aws.fix,
        "on the controller host, run `crabbox doctor --provider aws`"
    );
    assert_eq!(st.status, Status::Ok);
    assert_eq!(st.detail, "static: ready on the controller");
}

#[test]
fn a_failed_request_is_one_warning() {
    let report = rig(Err(ClientError::Protocol("x".to_owned()))).with(provider::controller);
    let found = lines(&report);
    assert_eq!(found.len(), 1);
    assert_eq!(found.first().map(|c| c.status), Some(Status::Warn));
}

#[test]
fn no_controller_adds_no_crabbox_line() {
    assert!(Rig::new().with(provider::controller).is_empty());
}

#[test]
fn the_error_class_shows_beside_the_check() {
    let r = rig(Ok(json!({"providers": [
        {"provider": "aws", "ok": false, "failing": [
            {"check": "provider", "status": "failed", "class": "config", "hint": "check_it"}]}
    ]})));
    let report = r.with(provider::controller);
    let found = lines(&report);
    assert_eq!(
        found.first().map(|c| c.detail.as_str()),
        Some("aws: provider [config] (check_it)")
    );
}
