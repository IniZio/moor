//! Tests for request shaping.

use super::*;
use crate::config::proxy::resolve::validate_allow;

const REPO: (&str, &str) = ("IniZio", "moor-pilot-example");

fn shape(method: &str, target: &str) -> Option<String> {
    request_shape(method, target, REPO)
}

fn is(method: &str, target: &str, want: &str) {
    assert_eq!(shape(method, target).as_deref(), Some(want), "{target}");
}

#[test]
fn repo_and_number_become_slots() {
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/pulls/12",
        "GET /repos/{repo}/pulls/*",
    );
}

#[test]
fn oids_become_wildcards() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    is(
        "GET",
        &format!("/repos/IniZio/moor-pilot-example/commits/{sha}/status"),
        "GET /repos/{repo}/commits/*/status",
    );
}

#[test]
fn other_repo_stays_literal() {
    is(
        "GET",
        "/repos/IniZio/other/pulls/3",
        "GET /repos/IniZio/other/pulls/*",
    );
}

#[test]
fn branch_names_and_file_paths_never_appear() {
    for target in [
        "/repos/IniZio/moor-pilot-example/branches/secret-branch/protection",
        "/repos/IniZio/moor-pilot-example/contents/src/secret.rs",
        "/repos/IniZio/moor-pilot-example/git/refs/heads/feat/secret",
        "/repos/IniZio/moor-pilot-example/compare/main...secret",
        "/repos/IniZio/moor-pilot-example/actions/workflows/secret.yml/runs",
    ] {
        let got = shape("GET", target).unwrap_or_default();
        assert!(!got.contains("secret") && !got.contains("main"), "{got}");
    }
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/contents/src/a.rs",
        "GET /repos/{repo}/contents/**",
    );
}

#[test]
fn free_tail_without_name_stays_plain() {
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/branches",
        "GET /repos/{repo}/branches",
    );
}

#[test]
fn refs_keep_heads_and_tags() {
    is(
        "PATCH",
        "/repos/IniZio/moor-pilot-example/git/refs/heads/feat/x",
        "PATCH /repos/{repo}/git/refs/heads/**",
    );
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/git/ref/heads/x",
        "GET /repos/{repo}/git/ref/heads/**",
    );
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/git/matching-refs/tags/v1",
        "GET /repos/{repo}/git/matching-refs/tags/**",
    );
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/git/refs/heads",
        "GET /repos/{repo}/git/refs/heads",
    );
    is(
        "POST",
        "/repos/IniZio/moor-pilot-example/git/refs",
        "POST /repos/{repo}/git/refs",
    );
}

#[test]
fn resource_names_stay_literal() {
    is(
        "POST",
        "/repos/IniZio/moor-pilot-example/forks",
        "POST /repos/{repo}/forks",
    );
    is("GET", "/user", "GET /user");
    is("GET", "/user/repos", "GET /user/repos");
    is("GET", "/repos/IniZio", "GET /repos/IniZio");
}

#[test]
fn actions_ids_and_workflow_names_become_wildcards() {
    let rerun = shape(
        "POST",
        "/repos/IniZio/moor-pilot-example/actions/runs/123/rerun",
    );
    assert_eq!(
        rerun.as_deref(),
        Some("POST /repos/{repo}/actions/runs/*/rerun")
    );
    let token = shape(
        "POST",
        "/repos/IniZio/moor-pilot-example/actions/runners/registration-token",
    );
    assert_eq!(
        token.as_deref(),
        Some("POST /repos/{repo}/actions/runners/registration-token")
    );
    assert_ne!(rerun, token);
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/actions/workflows/ci.yml/runs",
        "GET /repos/{repo}/actions/workflows/*/runs",
    );
}

#[test]
fn query_and_fragment_are_dropped() {
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/pulls?state=open&per_page=5#f",
        "GET /repos/{repo}/pulls",
    );
}

#[test]
fn path_is_normalised() {
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/",
        "GET /repos/{repo}",
    );
    is(
        "GET",
        "//repos/./x/../IniZio/moor-pilot-example",
        "GET /repos/{repo}",
    );
    is(
        "GET",
        "/repos/%49niZio/moor-pilot-example",
        "GET /repos/{repo}",
    );
}

#[test]
fn unsafe_segments_are_only_allowed_in_free_positions() {
    for target in [
        "/repos/o/n/pulls/%7Bx%7D",
        "/repos/a%2Ab/n",
        "/repos/a%3Fb/n",
        "/repos/a%20b/n",
    ] {
        assert_eq!(shape("GET", target), None, "{target}");
    }
    is(
        "GET",
        "/repos/o/n/actions/workflows/%7Bx%7D",
        "GET /repos/o/n/actions/workflows/*",
    );
    is(
        "GET",
        "/repos/o/n/contents/a%20b/%2A",
        "GET /repos/o/n/contents/**",
    );
}

#[test]
fn root_has_a_shape() {
    is("GET", "/", "GET /");
    is("GET", "", "GET /");
    is("GET", "/a/..", "GET /");
}

#[test]
fn unmatchable_requests_have_no_shape() {
    for target in [
        "/repos/IniZio%2Fmoor-pilot-example/pulls",
        "/repos/a%2eb",
        "/repos/a;b",
        "/repos/a\\b",
        "/repos/a%2541",
        "/repos/a%00b",
        "/repos/a%zz",
    ] {
        assert_eq!(shape("GET", target), None, "{target}");
    }
    for method in ["", "get", "G3T"] {
        assert_eq!(shape(method, "/x"), None, "{method}");
    }
}

#[test]
fn outputs_parse_as_allow_entries() {
    for target in [
        "/repos/IniZio/moor-pilot-example/pulls/12?per_page=1",
        "/repos/IniZio/moor-pilot-example",
        "/repos/IniZio/other/commits/deadbeef1/status",
        "/repos/IniZio/moor-pilot-example/contents/a/b",
        "/",
    ] {
        let entry = shape("POST", target).unwrap_or_default();
        let r = validate_allow(std::slice::from_ref(&entry), "t");
        assert!(r.is_ok(), "{entry}: {r:?}");
    }
}

#[test]
fn plain_ref_kinds_stay_literal() {
    is(
        "PATCH",
        "/repos/IniZio/moor-pilot-example/git/refs/notes/x",
        "PATCH /repos/{repo}/git/refs/notes/**",
    );
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/git/refs/pull/1/head",
        "GET /repos/{repo}/git/refs/pull/**",
    );
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/git/matching-refs/feature",
        "GET /repos/{repo}/git/matching-refs/**",
    );
}

#[test]
fn public_key_stays_literal_under_secrets() {
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/actions/secrets/public-key",
        "GET /repos/{repo}/actions/secrets/public-key",
    );
    is(
        "PUT",
        "/repos/IniZio/moor-pilot-example/actions/secrets/TOKEN",
        "PUT /repos/{repo}/actions/secrets/*",
    );
}

#[test]
fn short_hex_words_stay_literal() {
    is(
        "GET",
        "/repos/IniZio/moor-pilot-example/commits/abc123",
        "GET /repos/{repo}/commits/abc123",
    );
}

#[test]
fn double_encoded_percent_has_no_shape() {
    assert_eq!(shape("GET", "/repos/o/n/contents/a%2541"), None);
}
