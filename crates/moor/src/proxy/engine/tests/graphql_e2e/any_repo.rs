//! The default `github-api` rule through nono: reads of a Box repo get the credential, reads of
//! any other repo do not.

use moor_core::config::proxy::{resolve_rules, ProxyConfig};
use moor_core::seam::forge::ForgeRepo;
use ureq::http::Request;

use super::rejected::events_after;
use super::{rig_spec, Rig, Seen, PLACEHOLDER, SECRET};

/// A running Box on `o/r`, started with `--also o/lib`, with the default `github-api` rule.
pub(super) fn default_rule_rig() -> Rig {
    let repo = ForgeRepo::new("o", "r");
    let also = ForgeRepo::new("o", "lib");
    let rules = resolve_rules(
        &ProxyConfig::default(),
        &[repo.clone(), also.clone()],
        false,
    )
    .unwrap()
    .into_iter()
    .filter(|r| r.name == "github-api")
    .collect();
    rig_spec(repo, vec![also], rules, None)
}

/// Sends `method path` to `api.github.com`; what the fake upstream received, the raw path intact.
pub(super) fn send(rig: &Rig, method: &str, path: &str, authorization: Option<&str>) -> Seen {
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

/// GETs on a repo's metadata, releases, tags and contents.
const READS: &[&str] = &[
    "",
    "?per_page=1",
    "/",
    "/releases",
    "/releases?per_page=100",
    "/releases/latest",
    "/releases/tags/v1",
    "/releases/assets/5",
    "/tags",
    "/tags?per_page=100",
    "/contents",
    "/contents/",
    "/contents/README.md",
    "/contents/src/lib.rs?ref=main",
];

#[test]
fn a_get_on_a_box_repo_gets_the_credential_with_or_without_a_token() {
    let rig = default_rule_rig();
    let token = format!("token {PLACEHOLDER}");
    let paths = ["/repos/o/r", "/repos/o/lib"]
        .iter()
        .flat_map(|repo| READS.iter().map(move |read| format!("{repo}{read}")));
    for path in paths {
        let path = path.as_str();
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
fn a_get_on_another_repo_gets_no_credential() {
    let rig = default_rule_rig();
    for read in READS {
        for authorization in [None, Some("token gho_moor-proxy")] {
            let path = format!("/repos/cli/cli{read}");
            let seen = send(&rig, "GET", &path, authorization);
            assert_eq!(seen.authorization, None, "{path}: {}", seen.head);
            assert!(!seen.head.contains(SECRET), "{path}: {}", seen.head);
        }
    }
    rig.stop();
}

#[test]
fn a_write_on_a_box_repo_read_path_gets_no_credential() {
    let rig = default_rule_rig();
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        for path in [
            "/repos/o/lib",
            "/repos/o/lib/releases",
            "/repos/o/lib/releases/1",
            "/repos/o/lib/tags",
            "/repos/o/lib/contents/x",
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
        "/repos/cli/cli/actions/secrets",
        "/repos/cli/cli/contents/../pulls",
        "/repos/cli/cli/contents-x/y",
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
        "/repos/cli/cli/releases/../../../../user/emails",
        "/repos/cli%2Fcli",
        "/repos/cli/cli%2Fpulls",
        "/repos/cli/cli/releases%2F..%2Fpulls",
        "/repos/cli/cli/releases/%2e%2e/pulls",
        "/repos/cli/cli/releases/1%252F..%252Fpulls",
        "/repos/cli/cli/releases;/1",
        "/repos/cli/cli/pulls?x=/releases/1",
        "/repos/cli/cli/pulls?/../releases/1",
        "/user/emails",
        "/user/repos",
        "/user/../repos/cli/cli/pulls",
    ] {
        assert_uncredentialed(&rig, "GET", path);
    }
    rig.stop();
}

#[test]
fn a_get_on_user_gets_the_credential_and_nothing_under_it_does() {
    let rig = default_rule_rig();
    let seen = send(&rig, "GET", "/user", Some("token gho_moor-proxy"));
    let want = format!("token {SECRET}");
    assert_eq!(seen.authorization.as_deref(), Some(want.as_str()));
    for method in ["PATCH", "DELETE"] {
        assert_uncredentialed(&rig, method, "/user");
    }
    rig.stop();
}

#[test]
fn only_a_request_left_unmatched_is_reported_as_withheld() {
    let rig = default_rule_rig();
    for path in ["/repos/o/r", "/repos/o/lib/releases/latest"] {
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
