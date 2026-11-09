//! `/repositories/{id}/...`, the form GitHub's pagination links use: a Box repo's id gets the
//! credential for exactly the paths its `/repos/{owner}/{name}/...` form would.

use moor_core::config::proxy::{resolve_rules, EndpointRule, ProxyConfig};
use moor_core::seam::forge::ForgeRepo;

use super::any_repo::{default_rule_rig, send};
use super::rejected::events_after;
use super::{rig_spec, Rig, SECRET};

/// Sends `GET path` with the Box placeholder and asserts the credential went with it.
fn assert_credentialed(rig: &Rig, path: &str) {
    let seen = send(rig, "GET", path, Some("token gho_moor-proxy"));
    let want = format!("token {SECRET}");
    assert_eq!(seen.authorization.as_deref(), Some(want.as_str()), "{path}");
}

/// Sends `method path` with the Box placeholder and asserts the credential stayed behind.
fn assert_uncredentialed(rig: &Rig, method: &str, path: &str) {
    let seen = send(rig, method, path, Some("token gho_moor-proxy"));
    let secret = format!("token {SECRET}");
    assert_ne!(
        seen.authorization.as_deref(),
        Some(secret.as_str()),
        "{method} {path}"
    );
    assert!(
        !seen.head.contains(SECRET),
        "{method} {path}: {}",
        seen.head
    );
}

#[test]
fn a_box_repo_id_with_an_allowed_path_gets_the_credential() {
    let rig = default_rule_rig();
    for path in [
        "/repositories/101/dependabot/alerts?per_page=100&page=2",
        "/repositories/101/dependabot/alerts/7",
        "/repositories/101",
        "/repositories/101/pulls?page=3",
        "/repositories/102/dependabot/alerts?page=2",
        "/repositories/102/releases",
    ] {
        assert_credentialed(&rig, path);
    }
    rig.stop();
}

#[test]
fn another_repo_id_gets_no_credential() {
    let rig = default_rule_rig();
    for path in [
        "/repositories/900/dependabot/alerts?page=2",
        "/repositories/900",
        "/repositories/900/releases",
        "/repositories/555/pulls",
    ] {
        assert_uncredentialed(&rig, "GET", path);
    }
    rig.stop();
}

#[test]
fn a_box_repo_id_with_a_path_its_repos_form_denies_gets_no_credential() {
    let rig = default_rule_rig();
    for (method, path) in [
        ("GET", "/repositories/101/hooks"),
        ("GET", "/repositories/101/dependabot/secrets"),
        ("GET", "/repositories/101/dependabot/alerts/7/extra"),
        ("PATCH", "/repositories/101/dependabot/alerts/7"),
        ("POST", "/repositories/101/releases"),
        ("PUT", "/repositories/101/pulls/1/merge"),
        ("GET", "/repositories/101/../900/pulls"),
        ("GET", "/repositories/101/../900"),
        ("GET", "/repositories/101/pulls/../../900/pulls"),
        ("GET", "/repositories/101%2F..%2F900/pulls"),
        ("GET", "/repositories/0101/pulls"),
        ("GET", "/repositories/101x/pulls"),
    ] {
        assert_uncredentialed(&rig, method, path);
    }
    rig.stop();
}

#[test]
fn an_entry_naming_one_box_repo_credentials_only_that_repos_id() {
    let (repo, also) = (ForgeRepo::new("o", "r"), ForgeRepo::new("o", "lib"));
    let mut rules: Vec<_> = resolve_rules(
        &ProxyConfig::default(),
        &[repo.clone(), also.clone()],
        false,
    )
    .unwrap()
    .into_iter()
    .filter(|r| r.name == "github-api")
    .collect();
    for rule in &mut rules {
        rule.endpoint_rules.push(EndpointRule {
            method: "GET".to_owned(),
            path: "/repos/o/lib/hooks".to_owned(),
        });
    }
    let rig: Rig = rig_spec(repo, vec![also], rules, None);
    assert_credentialed(&rig, "/repositories/102/hooks");
    assert_uncredentialed(&rig, "GET", "/repositories/101/hooks");
    rig.stop();
}

#[test]
fn a_withheld_repo_id_is_reported_with_its_id_form_shape() {
    let rig = default_rule_rig();
    assert_uncredentialed(&rig, "GET", "/repositories/900/pulls?page=2");
    let events = events_after(&rig, "credential_withheld");
    let withheld: Vec<&str> = events
        .iter()
        .filter(|(k, _)| k == "credential_withheld")
        .map(|(_, p)| p.as_str())
        .collect();
    let [only] = withheld.as_slice() else {
        unreachable!("one report expected: {events:?}")
    };
    assert!(only.contains("GET /repositories/*/pulls"), "{only}");
    assert!(!only.contains("900"), "{only}");
    rig.stop();
}
