//! The user, github and claude checks.

use moor_core::user::Identity;
use moor_core::user::{SecretName, UserId};
use moor_core::view::user::{SecretNamesValue, StartNeedsValue};

use serde_json::json;

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
fn without_a_controller_the_user_line_warns() {
    let report = Rig::new().with(user::checks);
    assert_eq!(report.len(), 1);
    assert!(report.iter().all(|c| c.status == Status::Warn));
}

#[test]
fn a_user_who_is_not_enrolled_fails_and_stops_there() {
    let report = rig(None, &[], &[], &[]).with(user::checks);
    let [only] = report.as_slice() else {
        return assert_eq!(report.len(), 1);
    };
    assert_eq!(only.status, Status::Fail);
    assert_eq!(only.detail, "not enrolled");
    assert_eq!(only.fix, "run `moor user enroll`");
}

#[test]
fn an_enrolled_user_is_ok_and_each_need_has_its_own_line() {
    let mut r = rig(Some(1), &["claude", "github"], &["github"], &["claude"]);
    r.public_url = Some("https://moor.example/".to_owned());
    let report = r.with(user::checks);
    let lines: Vec<_> = report
        .iter()
        .map(|c| (c.name, c.status, c.detail.as_str()))
        .collect();
    assert_eq!(
        lines,
        [
            ("user", Status::Ok, "enrolled"),
            ("secret", Status::Ok, "claude: set"),
            ("secret", Status::Fail, "github: not set"),
        ]
    );
    let fix = report.last().map(|c| c.fix.as_str()).unwrap_or_default();
    assert!(fix.contains("moor user secret set github"), "{fix}");
    assert!(fix.contains("https://moor.example/me/secrets"), "{fix}");
}

#[test]
fn without_a_public_url_only_the_cli_way_is_offered() {
    let report = rig(Some(1), &["github"], &["github"], &[]).with(user::checks);
    assert_eq!(
        report.last().map(|c| c.fix.as_str()),
        Some("run `moor user secret set github` on the host")
    );
}

#[test]
fn a_need_that_is_missing_is_named_once_each() {
    let report = rig(Some(1), &["a", "b"], &["a", "b"], &[]).with(user::checks);
    let fixes: Vec<_> = report.iter().skip(1).map(|c| c.fix.as_str()).collect();
    let [a, b] = fixes.as_slice() else {
        return assert_eq!(fixes.len(), 2);
    };
    assert!(a.contains("secret set a"));
    assert!(b.contains("secret set b"));
}

#[test]
fn start_needs_carries_the_checkout_repo() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = std::rc::Rc::clone(&seen);
    let mut r = rig(Some(1), &[], &[], &[]);
    r.script = Box::new(move |req| {
        log.borrow_mut().push(req);
        answered(
            "start-needs",
            json!({"identity": "local:1000", "user": 1, "needs": [], "missing": []}),
        )
    });
    r.repo = Some(moor_core::name::ForgeRepo::parse("acme/demo").unwrap());
    r.with(user::checks);
    let sent = seen.borrow().first().cloned();
    let Some(Request::StartNeeds(req)) = sent else {
        return assert!(sent.is_some(), "no start-needs sent");
    };
    assert_eq!(
        req.repo.map(|r| r.to_string()),
        Some("acme/demo".to_owned())
    );
}

#[test]
fn a_controller_that_fails_start_needs_fails_the_user_check() {
    let r = Rig::new().with_controller(Box::new(|_| Err(ClientError::Protocol("x".to_owned()))));
    let report = r.with(user::checks);
    assert_eq!(report.len(), 1);
    assert_eq!(report.first().map(|c| c.status), Some(Status::Fail));
}
