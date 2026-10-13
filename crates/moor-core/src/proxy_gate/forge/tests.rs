//! Tests for the forge allow-lists.

use super::{admit_api, admit_git};
use crate::proxy_gate::{
    decide, Allow, GateDecision, GateInput, Inject, Plan, Reject, RouteId, RouteRow, RouteTable,
};
use crate::secret::Secret;

const REPO: Option<&str> = Some("acme/widget");
const UPLOAD: Option<&str> = Some("service=git-upload-pack");
const NONCE: &str = "q3Jt0bXo1n9Zr2Kc8vWm4yHs7aLp6eTg5uDf0iBk-_A";

fn git(method: &str, rest: &str, query: Option<&str>) -> Result<(), Reject> {
    admit_git(method, rest, query, REPO)
}

fn api(method: &str, rest: &str, query: Option<&str>) -> Result<(), Reject> {
    admit_api(method, rest, query, REPO)
}

fn forbidden(result: Result<(), Reject>) -> bool {
    matches!(result, Err(Reject::Forbidden(_)))
}

#[test]
fn constructors_trim_prefix_and_set_policy_and_inject() {
    let git = RouteRow::forge_git("/github/");
    assert_eq!(git.prefix, "/github");
    assert_eq!(git.id, RouteId::new("forge.git"));
    assert_eq!(git.allow, Allow::GitSmartHttp);
    assert_eq!(
        git.inject,
        Inject::Basic {
            user: "x-access-token"
        }
    );
    assert!(git.strip.is_empty());
    let api = RouteRow::forge_api(String::from("/github-api"));
    assert_eq!(api.prefix, "/github-api");
    assert_eq!(api.id, RouteId::new("forge.api"));
    assert_eq!(api.allow, Allow::PullsApi);
    assert_eq!(api.inject, Inject::Bearer);
}

#[test]
fn git_admits_advertisement_for_both_services() {
    for method in ["GET", "HEAD"] {
        for service in ["service=git-upload-pack", "service=git-receive-pack"] {
            assert_eq!(
                git(method, "/acme/widget.git/info/refs", Some(service)),
                Ok(())
            );
            assert_eq!(git(method, "/acme/widget/info/refs", Some(service)), Ok(()));
        }
    }
}

#[test]
fn git_admits_service_posts() {
    for service in ["git-upload-pack", "git-receive-pack"] {
        assert_eq!(
            git("POST", &format!("/acme/widget.git/{service}"), None),
            Ok(())
        );
        assert_eq!(
            git("POST", &format!("/acme/widget/{service}"), None),
            Ok(())
        );
    }
}

#[test]
fn git_repo_comparison_ignores_case_and_dot_git_on_both_sides() {
    assert_eq!(
        git("POST", "/ACME/Widget.git/git-upload-pack", None),
        Ok(())
    );
    assert_eq!(
        admit_git(
            "POST",
            "/acme/widget/git-upload-pack",
            None,
            Some("Acme/Widget.git")
        ),
        Ok(())
    );
}

#[test]
fn git_wrong_method_is_bad_method() {
    assert_eq!(
        git("POST", "/acme/widget/info/refs", UPLOAD),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        git("PUT", "/acme/widget/info/refs", UPLOAD),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        git("GET", "/acme/widget/git-upload-pack", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        git("DELETE", "/acme/widget/git-receive-pack", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        git("get", "/acme/widget/info/refs", UPLOAD),
        Err(Reject::BadMethod)
    );
}

#[test]
fn git_bad_service_query_is_forbidden() {
    for query in [
        None,
        Some(""),
        Some("service=git-other"),
        Some("service=git-upload-pack&x=1"),
        Some("x=1&service=git-upload-pack"),
        Some("service=GIT-UPLOAD-PACK"),
    ] {
        assert!(
            forbidden(git("GET", "/acme/widget/info/refs", query)),
            "{query:?}"
        );
    }
}

#[test]
fn git_post_with_query_is_forbidden() {
    assert!(forbidden(git(
        "POST",
        "/acme/widget/git-upload-pack",
        UPLOAD
    )));
    assert!(forbidden(git(
        "POST",
        "/acme/widget/git-receive-pack",
        Some("")
    )));
}

#[test]
fn git_unknown_paths_are_forbidden() {
    for rest in [
        "/acme/widget/objects/info/packs",
        "/acme/widget/HEAD",
        "/acme/widget/info/refs/extra",
        "/acme/widget/git-upload-pack/extra",
        "/acme/widget.git",
        "/acme/widget",
        "/acme",
        "/",
        "",
        "acme/widget/info/refs",
        "/acme/widget/info",
    ] {
        assert!(forbidden(git("GET", rest, UPLOAD)), "{rest}");
        assert!(forbidden(git("POST", rest, None)), "{rest}");
    }
}

#[test]
fn other_repositories_are_forbidden() {
    for rest in [
        "/acme/other/info/refs",
        "/other/widget/info/refs",
        "/acme/widget2/info/refs",
        "/acme/widget.git.git/info/refs",
        "/acme/.git/info/refs",
    ] {
        assert!(forbidden(git("GET", rest, UPLOAD)), "{rest}");
    }
    assert!(forbidden(api("GET", "/repos/acme/other/pulls", None)));
    assert!(forbidden(api("GET", "/repos/other/widget", None)));
}

#[test]
fn missing_or_malformed_task_repo_is_forbidden() {
    for task_repo in [
        None,
        Some(""),
        Some("acme"),
        Some("acme/"),
        Some("/widget"),
        Some("acme/widget/extra"),
        Some("acme/.git"),
        Some("acme/wid get"),
        Some("acme/.."),
    ] {
        assert!(
            forbidden(admit_git(
                "GET",
                "/acme/widget/info/refs",
                UPLOAD,
                task_repo
            )),
            "{task_repo:?}"
        );
        assert!(
            forbidden(admit_api("GET", "/repos/acme/widget", None, task_repo)),
            "{task_repo:?}"
        );
    }
}

#[test]
fn traversal_encoding_and_slash_abuse_are_forbidden() {
    for rest in [
        "/acme/widget/../widget/info/refs",
        "/../acme/widget/info/refs",
        "/acme/../acme/widget/info/refs",
        "/acme/widget/info/refs/..",
        "/acme/%2e%2e/acme/widget/info/refs",
        "/acme/%2E%2E/acme/widget/info/refs",
        "/acme/widget/info/%72efs",
        "/acme/wid%67et/info/refs",
        "//acme/widget/info/refs",
        "/acme//widget/info/refs",
        "/acme/widget/info/refs/",
        "/acme/widget\\info/refs",
    ] {
        assert!(forbidden(git("GET", rest, UPLOAD)), "{rest}");
    }
    for rest in [
        "/repos/acme/widget/../widget/pulls",
        "/repos/acme/%2e%2e/acme/widget/pulls",
        "/repos/acme/%2E%2E/acme/widget/pulls",
        "//repos/acme/widget/pulls",
        "/repos//acme/widget/pulls",
        "/repos/acme/widget/pulls/",
        "/repos/acme/widget/",
    ] {
        assert!(forbidden(api("GET", rest, None)), "{rest}");
    }
}

#[test]
fn api_admits_the_pull_request_forms() {
    assert_eq!(api("GET", "/repos/acme/widget/pulls", None), Ok(()));
    assert_eq!(
        api(
            "GET",
            "/repos/acme/widget/pulls",
            Some("state=open&per_page=50&page=2")
        ),
        Ok(())
    );
    assert_eq!(api("POST", "/repos/acme/widget/pulls", None), Ok(()));
    assert_eq!(api("GET", "/repos/acme/widget/pulls/12", None), Ok(()));
    assert_eq!(api("GET", "/repos/acme/widget", None), Ok(()));
}

#[test]
fn api_repo_comparison_ignores_case_and_dot_git() {
    assert_eq!(api("GET", "/repos/ACME/Widget/pulls", None), Ok(()));
    assert_eq!(api("GET", "/repos/acme/widget.git/pulls/7", None), Ok(()));
    assert_eq!(
        admit_api(
            "POST",
            "/repos/acme/widget/pulls",
            None,
            Some("ACME/WIDGET.git")
        ),
        Ok(())
    );
}

#[test]
fn api_known_shapes_with_wrong_methods_are_bad_method() {
    assert_eq!(
        api("PATCH", "/repos/acme/widget/pulls/12", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("DELETE", "/repos/acme/widget/pulls/12", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("POST", "/repos/acme/widget/pulls/12", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("PUT", "/repos/acme/widget/pulls", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("DELETE", "/repos/acme/widget/pulls", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("POST", "/repos/acme/widget", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("DELETE", "/repos/acme/widget", None),
        Err(Reject::BadMethod)
    );
    assert_eq!(
        api("get", "/repos/acme/widget/pulls", None),
        Err(Reject::BadMethod)
    );
}

#[test]
fn api_unknown_paths_are_forbidden() {
    for rest in [
        "/repos/acme/widget/issues",
        "/repos/acme/widget/pulls/12/merge",
        "/repos/acme/widget/pulls/12/files",
        "/repos/acme/widget/pulls/abc",
        "/repos/acme/widget/pulls/12a",
        "/repos/acme/widget/pulls/-1",
        "/repos/acme/widget/pulls/1.5",
        "/repos/acme/widget/contents/README.md",
        "/repos/acme/widget/git/refs",
        "/repos/acme/widget/pulls/12/comments",
        "/repos/acme",
        "/repos",
        "/user",
        "/graphql",
        "/acme/widget/pulls",
        "/orgs/acme/repos",
        "/",
        "",
    ] {
        assert!(forbidden(api("GET", rest, None)), "{rest}");
        assert!(forbidden(api("POST", rest, None)), "{rest}");
    }
}

#[test]
fn api_non_digit_pull_number_with_wrong_method_stays_forbidden() {
    assert!(forbidden(api(
        "PATCH",
        "/repos/acme/widget/pulls/abc",
        None
    )));
}

#[test]
fn api_queries_are_only_admitted_on_the_list() {
    assert!(forbidden(api(
        "POST",
        "/repos/acme/widget/pulls",
        Some("x=1")
    )));
    assert!(forbidden(api(
        "GET",
        "/repos/acme/widget/pulls/12",
        Some("x=1")
    )));
    assert!(forbidden(api("GET", "/repos/acme/widget", Some("x=1"))));
    assert!(forbidden(api("GET", "/repos/acme/widget", Some(""))));
}

fn forwarded(decision: GateDecision) -> Plan {
    match decision {
        GateDecision::Forward(plan) => Some(plan),
        GateDecision::Reject(_) => None,
    }
    .unwrap()
}

fn table() -> RouteTable {
    RouteTable::new(vec![
        RouteRow::model(),
        RouteRow::forge_git("/github/"),
        RouteRow::forge_api("/github-api/"),
    ])
}

fn run(method: &str, target: &str, task_repo: Option<&str>) -> GateDecision {
    let table = table();
    let secret = Secret::new(NONCE.as_bytes().to_vec());
    let auth = format!("Bearer {NONCE}");
    decide(&GateInput {
        table: &table,
        method,
        target,
        headers: &[("authorization", &auth)],
        nonce: &secret,
        task_repo,
    })
}

#[test]
fn gate_forwards_task_repo_git_and_api_calls() {
    let planned = forwarded(run(
        "GET",
        "/github/acme/widget.git/info/refs?service=git-upload-pack",
        Some("acme/widget"),
    ));
    assert_eq!(planned.route, RouteId::new("forge.git"));
    assert_eq!(
        planned.inject,
        Inject::Basic {
            user: "x-access-token"
        }
    );
    let planned = forwarded(run(
        "POST",
        "/github-api/repos/acme/widget/pulls",
        Some("acme/widget"),
    ));
    assert_eq!(planned.route, RouteId::new("forge.api"));
    assert_eq!(planned.inject, Inject::Bearer);
    assert_eq!(planned.upstream_path, "/github-api/repos/acme/widget/pulls");
}

#[test]
fn gate_refuses_other_repos_and_missing_task_repo() {
    assert_eq!(
        run(
            "GET",
            "/github-api/repos/acme/other/pulls",
            Some("acme/widget")
        ),
        GateDecision::Reject(Reject::Forbidden("repository is not the task repository"))
    );
    assert_eq!(
        run("POST", "/github/acme/widget/git-receive-pack", None),
        GateDecision::Reject(Reject::Forbidden("task has no forge repository"))
    );
}

#[test]
fn gate_maps_wrong_methods_and_hostile_paths() {
    assert_eq!(
        run(
            "PATCH",
            "/github-api/repos/acme/widget/pulls/12",
            Some("acme/widget")
        ),
        GateDecision::Reject(Reject::BadMethod)
    );
    let hostile = run(
        "GET",
        "/github/acme/%2e%2e/acme/widget/info/refs",
        Some("acme/widget"),
    );
    assert!(matches!(
        hostile,
        GateDecision::Reject(Reject::Forbidden(_))
    ));
}

#[test]
fn gate_still_sends_everything_else_to_the_model_route() {
    let planned = forwarded(run("POST", "/v1/messages", Some("acme/widget")));
    assert_eq!(planned.route, RouteId::model());
}
