//! The default `github-api` rule through nono: reads of any repo get the credential, nothing else does.

use moor_core::config::proxy::{resolve_rules, ProxyConfig};
use moor_core::seam::forge::ForgeRepo;
use ureq::http::Request;

use super::rejected::events_after;
use super::{rig_spec, Rig, Seen, PLACEHOLDER, SECRET};

/// A running Box on `o/r` with the default `github-api` rule.
fn default_rule_rig() -> Rig {
    let repo = ForgeRepo::new("o", "r");
    let rules = resolve_rules(&ProxyConfig::default(), std::slice::from_ref(&repo), false)
        .unwrap()
        .into_iter()
        .filter(|r| r.name == "github-api")
        .collect();
    rig_spec(repo, Vec::new(), rules, None)
}

/// Sends `method path` to `api.github.com`; what the fake upstream received, the raw path intact.
fn send(rig: &Rig, method: &str, path: &str, authorization: Option<&str>) -> Seen {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("https://api.github.com{path}"));
    if let Some(value) = authorization {
        request = request.header("Authorization", value);
    }
    rig.client().run(request.body(()).unwrap()).unwrap();
    let seen = rig.seen().pop().unwrap();
    assert_eq!(seen.line, format!("{method} {path} HTTP/1.1"));
    seen
}

/// Sends `method path` with the Box placeholder and asserts the credential stayed behind.
fn assert_uncredentialed(rig: &Rig, method: &str, path: &str) {
    let seen = send(rig, method, path, Some("token gho_moor-proxy"));
    assert_eq!(seen.authorization, None, "{method} {path}: {}", seen.head);
    assert!(
        !seen.head.contains(SECRET),
        "{method} {path}: {}",
        seen.head
    );
}

#[test]
fn a_get_on_any_repo_gets_the_credential_with_or_without_a_token() {
    let rig = default_rule_rig();
    let token = format!("token {PLACEHOLDER}");
    for path in [
        "/repos/cli/cli",
        "/repos/cli/cli?per_page=1",
        "/repos/cli/cli/",
        "/repos/cli/cli/releases",
        "/repos/cli/cli/releases?per_page=100",
        "/repos/cli/cli/releases/latest",
        "/repos/cli/cli/releases/tags/v1",
        "/repos/cli/cli/releases/assets/5",
        "/repos/cli/cli/tags",
        "/repos/cli/cli/tags?per_page=100",
    ] {
        for authorization in [None, Some(token.as_str())] {
            let seen = send(&rig, "GET", path, authorization);
            let sent = seen
                .head
                .lines()
                .filter(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                .count();
            assert_eq!(sent, 1, "{path}: {}", seen.head);
            let want = format!("token {SECRET}");
            assert_eq!(seen.authorization.as_deref(), Some(want.as_str()), "{path}");
            assert!(!seen.head.contains(PLACEHOLDER), "{path}: {}", seen.head);
        }
    }
    rig.stop();
}

#[test]
fn a_write_on_an_any_repo_path_gets_no_credential() {
    let rig = default_rule_rig();
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        for path in [
            "/repos/cli/cli",
            "/repos/cli/cli/releases",
            "/repos/cli/cli/releases/1",
            "/repos/cli/cli/tags",
        ] {
            assert_uncredentialed(&rig, method, path);
        }
    }
    rig.stop();
}

#[test]
fn other_endpoints_and_confusing_paths_get_no_credential() {
    let rig = default_rule_rig();
    for path in [
        "/repos/cli/cli/pulls",
        "/repos/cli/cli/contents/x",
        "/repos/cli/cli/actions/secrets",
        "/repos/cli/cli/pulls/1/releases/1",
        "/repos/cli/cli/releases-x/1",
        "/repos/cli/cli/tags/1",
        "/repos/cli/cli/tags-x",
        "/repos/cli/cli/tags/../pulls",
        "/repos/cli/cli/tags%2F..%2Fpulls",
        "/repos/cli/cli/dependabot/alerts",
        "/repos/cli",
        "/repos/a/b/../../x",
        "/repos/cli/cli/releases/../pulls",
        "/repos/cli/cli/releases/1/../../actions/secrets",
        "/repos/cli/cli/releases/../../../../user",
        "/repos/cli%2Fcli",
        "/repos/cli/cli%2Fpulls",
        "/repos/cli/cli/releases%2F..%2Fpulls",
        "/repos/cli/cli/releases/%2e%2e/pulls",
        "/repos/cli/cli/releases/1%252F..%252Fpulls",
        "/repos/cli/cli/releases;/1",
        "/repos/cli/cli/pulls?x=/releases/1",
        "/repos/cli/cli/pulls?/../releases/1",
        "/user",
    ] {
        assert_uncredentialed(&rig, "GET", path);
    }
    rig.stop();
}

#[test]
fn only_a_request_left_unmatched_is_reported_as_withheld() {
    let rig = default_rule_rig();
    for path in ["/repos/cli/cli", "/repos/cli/cli/releases/latest"] {
        send(&rig, "GET", path, Some("token gho_moor-proxy"));
    }
    assert_uncredentialed(&rig, "GET", "/repos/cli/cli/pulls");
    let events = events_after(&rig, "credential_withheld");
    let withheld: Vec<&str> = events
        .iter()
        .filter(|(k, _)| k == "credential_withheld")
        .map(|(_, p)| p.as_str())
        .collect();
    let [only] = withheld.as_slice() else {
        unreachable!("one report expected: {events:?}")
    };
    assert!(only.contains("/repos/cli/cli/pulls"), "{only}");
    rig.stop();
}
