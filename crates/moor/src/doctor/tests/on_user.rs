//! The user, github and claude checks.

use moor_core::user::Identity;
use moor_core::user::{SecretName, UserId};
use moor_core::view::user::{SecretNamesValue, StartNeedsValue};

use super::*;

fn name(s: &str) -> SecretName {
    SecretName::parse(s).unwrap()
}

/// A controller that answers `start-needs` and `secret-names`.
fn rig(user: Option<i64>, needs: &[&str], missing: &[&str], stored: &[&str]) -> Rig {
    let needs = StartNeedsValue {
        identity: Identity::parse("local:1000").unwrap(),
        user: user.map(UserId::new),
        needs: needs.iter().map(|n| name(n)).collect(),
        missing: missing.iter().map(|n| name(n)).collect(),
    };
    let names = SecretNamesValue {
        names: stored.iter().map(|n| name(n)).collect(),
    };
    Rig::new().with_controller(Box::new(move |r| match r {
        Request::StartNeeds(_) => answered("start-needs", serde_json::to_value(&needs).unwrap()),
        Request::SecretNames(_) => answered("secret-names", serde_json::to_value(&names).unwrap()),
        other => Err(ClientError::Usage(format!("unexpected {}", other.cmd()))),
    }))
}

#[test]
fn without_a_controller_all_three_warn() {
    let report = Rig::new().with(user::checks);
    assert_eq!(report.len(), 3);
    assert!(report.iter().all(|c| c.status == Status::Warn));
}

#[test]
fn a_user_who_is_not_enrolled_fails_and_stops_there() {
    let report = rig(None, &[], &[], &[]).with(user::checks);
    let [only] = report.as_slice() else {
        return assert_eq!(report.len(), 1);
    };
    assert_eq!(only.status, Status::Fail);
    assert_eq!(only.fix, "run `moor user enroll`");
}

#[test]
fn missing_secrets_are_named_with_both_ways_to_set_them() {
    let mut r = rig(Some(1), &["github", "claude"], &["github", "claude"], &[]);
    r.public_url = Some("https://moor.example/".to_owned());
    let report = r.with(user::checks);
    let u = named(&report, "user");
    assert_eq!(u.status, Status::Fail);
    assert_eq!(u.detail, "missing secrets: github, claude");
    assert_eq!(
        u.fix,
        "run `moor user secret set github` on the host, or open https://moor.example/me/secrets"
    );
    assert_eq!(named(&report, "github").status, Status::Fail);
    assert_eq!(named(&report, "claude").status, Status::Fail);
}

#[test]
fn without_a_public_url_only_the_cli_way_is_offered() {
    let report = rig(Some(1), &["github"], &["github"], &[]).with(user::checks);
    assert_eq!(
        named(&report, "user").fix,
        "run `moor user secret set github` on the host"
    );
}

#[test]
fn stored_secrets_pass_the_user_check_and_warn_that_their_values_are_sealed() {
    let report = rig(Some(1), &["github"], &[], &["github", "claude"]).with(user::checks);
    assert_eq!(named(&report, "user").status, Status::Ok);
    for secret in ["github", "claude"] {
        let c = named(&report, secret);
        assert_eq!(c.status, Status::Warn, "{c:?}");
        assert!(c.detail.contains("only the controller can open it"));
    }
}

#[test]
fn a_controller_that_fails_start_needs_fails_the_user_check() {
    let r = Rig::new().with_controller(Box::new(|_| Err(ClientError::Protocol("x".to_owned()))));
    let report = r.with(user::checks);
    assert_eq!(report.len(), 1);
    assert_eq!(report.first().map(|c| c.status), Some(Status::Fail));
}
