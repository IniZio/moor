//! Tests for the proxy gate.

use super::{
    decide, Allow, GateDecision, GateInput, Inject, Plan, Reject, RouteId, RouteRow, RouteTable,
    ALWAYS_STRIP,
};
use crate::secret::Secret;

const NONCE: &str = "q3Jt0bXo1n9Zr2Kc8vWm4yHs7aLp6eTg5uDf0iBk-_A";

fn nonce() -> Secret {
    Secret::new(NONCE.as_bytes().to_vec())
}

fn forge_row(prefix: &str) -> RouteRow {
    RouteRow {
        id: RouteId::new("forge.git"),
        prefix: prefix.into(),
        allow: Allow::AnyPath,
        strip: vec![],
        inject: Inject::None,
    }
}

fn model_only() -> RouteTable {
    RouteTable::new(vec![RouteRow::model()])
}

fn with_forge(prefix: &str) -> RouteTable {
    RouteTable::new(vec![RouteRow::model(), forge_row(prefix)])
}

fn run(
    table: &RouteTable,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    secret: &Secret,
) -> GateDecision {
    decide(&GateInput {
        table,
        method,
        target,
        headers,
        nonce: secret,
        task_repo: None,
    })
}

fn bearer() -> String {
    format!("Bearer {NONCE}")
}

fn authed(table: &RouteTable, target: &str) -> GateDecision {
    let auth = bearer();
    run(table, "GET", target, &[("authorization", &auth)], &nonce())
}

fn plan(decision: GateDecision) -> Plan {
    match decision {
        GateDecision::Forward(plan) => Some(plan),
        GateDecision::Reject(_) => None,
    }
    .unwrap()
}

fn header_names(plan: &Plan) -> Vec<&str> {
    plan.headers.iter().map(|(n, _)| n.as_str()).collect()
}

#[test]
fn targets_without_leading_slash_are_bad_requests() {
    for target in ["v1/messages", "http://x/", "*", ""] {
        let decision = authed(&model_only(), target);
        assert_eq!(
            decision,
            GateDecision::Reject(Reject::BadRequest),
            "{target}"
        );
    }
}

#[test]
fn path_nonce_authenticates_and_is_removed_from_upstream_path() {
    let table = model_only();
    let cases = [
        (format!("/t/{NONCE}"), "/"),
        (format!("/t/{NONCE}/"), "/"),
        (format!("/t/{NONCE}/v1/x?a=1"), "/v1/x?a=1"),
        (format!("/t/{NONCE}/v1/messages"), "/v1/messages"),
    ];
    for (target, upstream) in cases {
        let got = run(&table, "GET", &target, &[], &nonce());
        assert_eq!(plan(got).upstream_path, upstream, "{target}");
    }
}

#[test]
fn bad_path_nonce_is_unauthorized() {
    let table = model_only();
    let short = NONCE.strip_suffix('A').unwrap();
    let targets = [
        "/t//x".to_string(),
        "/t/".to_string(),
        "/t/wrong/x".to_string(),
        format!("/t/{short}/x"),
        format!("/t/{NONCE}A/x"),
    ];
    for target in targets {
        let got = run(&table, "GET", &target, &[], &nonce());
        assert_eq!(got, GateDecision::Reject(Reject::Unauthorized), "{target}");
    }
}

#[test]
fn bearer_header_authenticates_with_any_case() {
    let table = model_only();
    for (name, value) in [
        ("authorization", format!("Bearer {NONCE}")),
        ("Authorization", format!("Bearer {NONCE}")),
        ("AUTHORIZATION", format!("bearer {NONCE}")),
        ("authorization", format!("BEARER {NONCE}")),
    ] {
        let got = run(&table, "GET", "/v1/x?a=1", &[(name, &value)], &nonce());
        assert_eq!(plan(got).upstream_path, "/v1/x?a=1", "{name}: {value}");
    }
}

#[test]
fn bad_or_missing_credentials_are_unauthorized() {
    let table = model_only();
    let short = format!("Bearer {}", NONCE.strip_suffix('A').unwrap());
    let longer = format!("Bearer {NONCE}A");
    let basic = format!("Basic {NONCE}");
    let cases: Vec<Vec<(&str, &str)>> = vec![
        vec![],
        vec![("authorization", "Bearer wrong")],
        vec![("authorization", "Bearer ")],
        vec![("authorization", "Bearer")],
        vec![("authorization", &basic)],
        vec![("authorization", &short)],
        vec![("authorization", &longer)],
        vec![("x-api-key", NONCE)],
        vec![("authorization", "Bearer wrong"), ("x-api-key", NONCE)],
    ];
    for headers in cases {
        let got = run(&table, "GET", "/v1/x", &headers, &nonce());
        assert_eq!(
            got,
            GateDecision::Reject(Reject::Unauthorized),
            "{headers:?}"
        );
    }
}

#[test]
fn first_authorization_header_is_the_one_checked() {
    let good = bearer();
    let got = run(
        &model_only(),
        "GET",
        "/x",
        &[("authorization", "Bearer wrong"), ("authorization", &good)],
        &nonce(),
    );
    assert_eq!(got, GateDecision::Reject(Reject::Unauthorized));
}

#[test]
fn empty_expected_nonce_never_authenticates() {
    let empty = Secret::new(Vec::new());
    let table = model_only();
    let cases: [(&str, Vec<(&str, &str)>); 4] = [
        ("/x", vec![("authorization", "Bearer ")]),
        ("/x", vec![("authorization", "Bearer")]),
        ("/t//x", vec![]),
        ("/t/", vec![]),
    ];
    for (target, headers) in cases {
        let got = run(&table, "GET", target, &headers, &empty);
        assert_eq!(got, GateDecision::Reject(Reject::Unauthorized), "{target}");
    }
}

#[test]
fn longest_prefix_wins_and_model_takes_the_rest() {
    for prefix in ["/github", "/github/"] {
        let table = with_forge(prefix);
        let cases = [
            ("/github/o/r", "forge.git"),
            ("/github", "forge.git"),
            ("/github?x=1", "forge.git"),
            ("/githubx/foo", "model"),
            ("/github-api/repos", "model"),
            ("/", "model"),
            ("/v1/messages", "model"),
        ];
        for (target, route) in cases {
            let got = plan(authed(&table, target));
            assert_eq!(got.route.as_str(), route, "{prefix} {target}");
        }
    }
}

#[test]
fn path_nonce_requests_route_after_the_nonce_is_removed() {
    let table = with_forge("/github");
    let got = plan(run(
        &table,
        "GET",
        &format!("/t/{NONCE}/github/o/r"),
        &[],
        &nonce(),
    ));
    assert_eq!(got.route.as_str(), "forge.git");
    assert_eq!(got.upstream_path, "/github/o/r");
}

#[test]
fn empty_table_is_not_found() {
    let table = RouteTable::new(vec![]);
    assert_eq!(authed(&table, "/x"), GateDecision::Reject(Reject::NotFound));
}

#[test]
fn model_route_allows_any_method() {
    let table = model_only();
    let auth = bearer();
    for method in [
        "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "PROPFIND",
    ] {
        let got = run(
            &table,
            method,
            "/v1/messages",
            &[("authorization", &auth)],
            &nonce(),
        );
        let got = plan(got);
        assert_eq!(got.method, method);
        assert_eq!(got.route, RouteId::model());
        assert_eq!(got.inject, Inject::Bearer);
    }
}

#[test]
fn model_route_passes_any_path_through_unchanged() {
    for target in [
        "/",
        "/v1/messages",
        "/anything/deep?x=1",
        "/../etc/passwd",
        "/a/../b?q=/..",
    ] {
        assert_eq!(plan(authed(&model_only(), target)).upstream_path, target);
    }
}

#[test]
fn credential_and_hop_by_hop_headers_are_stripped_case_insensitively() {
    let auth = bearer();
    let mut headers: Vec<(String, String)> = vec![("Authorization".into(), auth)];
    for name in ALWAYS_STRIP {
        headers.push((name.to_uppercase(), "v".into()));
        headers.push((name.to_string(), "v".into()));
    }
    headers.push(("X-Api-Key".into(), "k".into()));
    headers.push(("HOST".into(), "evil".into()));
    headers.push(("content-type".into(), "application/json".into()));
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let got = plan(run(&model_only(), "POST", "/v1/messages", &refs, &nonce()));
    assert_eq!(
        got.headers,
        vec![("content-type".to_string(), "application/json".to_string())]
    );
}

#[test]
fn connection_listed_headers_are_stripped() {
    let auth = bearer();
    let headers = [
        ("authorization", auth.as_str()),
        ("Connection", "keep-alive, X-Foo"),
        ("x-foo", "1"),
        ("X-Bar", "2"),
    ];
    let got = plan(run(&model_only(), "GET", "/x", &headers, &nonce()));
    assert_eq!(header_names(&got), vec!["X-Bar"]);
}

#[test]
fn row_strip_names_are_removed_case_insensitively() {
    let mut row = forge_row("/github");
    row.strip = vec!["x-secret".into(), "Cookie".into()];
    let table = RouteTable::new(vec![RouteRow::model(), row]);
    let auth = bearer();
    let headers = [
        ("authorization", auth.as_str()),
        ("X-Secret", "s"),
        ("cookie", "c"),
        ("accept", "*/*"),
    ];
    let got = plan(run(&table, "GET", "/github/o/r", &headers, &nonce()));
    assert_eq!(header_names(&got), vec!["accept"]);
    let other = plan(run(&table, "GET", "/v1/x", &headers, &nonce()));
    assert_eq!(header_names(&other), vec!["X-Secret", "cookie", "accept"]);
}

#[test]
fn kept_headers_preserve_case_value_and_order() {
    let auth = bearer();
    let headers = [
        ("content-type", "application/json"),
        ("authorization", auth.as_str()),
        ("anthropic-version", "2023-06-01"),
        ("Mcp-Session-Id", "AbC123"),
    ];
    let got = plan(run(
        &model_only(),
        "POST",
        "/v1/messages",
        &headers,
        &nonce(),
    ));
    let expected: Vec<(String, String)> = [
        ("content-type", "application/json"),
        ("anthropic-version", "2023-06-01"),
        ("Mcp-Session-Id", "AbC123"),
    ]
    .iter()
    .map(|(n, v)| (n.to_string(), v.to_string()))
    .collect();
    assert_eq!(got.headers, expected);
}

#[test]
fn path_nonce_requests_keep_other_headers() {
    let headers = [("x-api-key", "k"), ("accept", "text/event-stream")];
    let target = format!("/t/{NONCE}/v1/x");
    let got = plan(run(&model_only(), "GET", &target, &headers, &nonce()));
    assert_eq!(header_names(&got), vec!["accept"]);
}

#[test]
fn debug_output_never_contains_the_nonce() {
    let by_path = run(
        &model_only(),
        "GET",
        &format!("/t/{NONCE}/x"),
        &[],
        &nonce(),
    );
    let by_header = authed(&model_only(), "/x");
    for decision in [by_path, by_header] {
        assert!(matches!(decision, GateDecision::Forward(_)));
        assert!(!format!("{decision:?}").contains(NONCE));
    }
}

#[test]
fn reject_statuses_map_to_http_codes() {
    assert_eq!(Reject::BadRequest.status(), 400);
    assert_eq!(Reject::Unauthorized.status(), 401);
    assert_eq!(Reject::Forbidden("nope").status(), 403);
    assert_eq!(Reject::NotFound.status(), 404);
    assert_eq!(Reject::BadMethod.status(), 405);
}
