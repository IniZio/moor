//! Which GraphQL requests reach the upstream, and what the upstream sees.

use super::*;
use crate::proxy::graphql::SCOPE_CATEGORY;

/// Whether `Wire` carries the real credential in its `Authorization` header.
fn has_token(w: &Wire) -> bool {
    w.header("authorization") == Some(&format!("token {TOKEN}"))
}

#[test]
fn task_repo_query_is_forwarded_verbatim_and_reply_returned_unchanged() {
    let reply = chunked_reply(&[r#"{"data":"#, r#"{"ok":"#, "true}}"]);
    let rig = rig(fixed(reply.clone()));
    let mut client = rig.client();
    let sent = post("/graphql", &repo_query("IniZio", "moor", "Q"));
    let resp = client.call(&sent);
    assert_eq!(resp.status(), 200);
    assert!(resp.raw == reply, "reply bytes changed");
    let seen = rig.requests();
    assert_eq!(seen.len(), 1);
    assert!(
        seen.first().is_some_and(|w| w.raw == sent),
        "request bytes changed"
    );
    assert!(seen.first().is_some_and(has_token));
    assert!(rig.refusals(0).is_empty());
}

#[test]
fn other_repo_query_is_refused_and_never_forwarded() {
    let rig = rig(standard(nodes_reply(&[])));
    let mut client = rig.client();
    let resp = client.call(&post("/graphql", &repo_query("victim", "secrets", "Q")));
    assert_refused(&resp, 403);
    assert!(client.closed());
    assert!(rig.requests().is_empty());
    let refusals = rig.refusals(1);
    assert_eq!(
        refusals,
        vec![Refusal {
            host: "api.github.com".to_owned(),
            rule: RULE.to_owned(),
            method: "POST".to_owned(),
            category: SCOPE_CATEGORY,
        }]
    );
    assert_eq!(SCOPE_CATEGORY, "graphql_scope");
}

#[test]
fn node_id_of_another_repo_is_refused_after_one_lookup() {
    let rig = rig(standard(nodes_reply(&["victim/secrets"])));
    let mut client = rig.client();
    let resp = client.call(&post("/graphql", &create_pr("R_other")));
    assert_refused(&resp, 403);
    let seen = rig.requests();
    assert_eq!(seen.len(), 1, "only the lookup may reach upstream");
    let lookup = seen.first().expect("lookup");
    assert!(lookup.is_lookup() && has_token(lookup));
    let body: Value = serde_json::from_slice(&lookup.body).unwrap();
    assert_eq!(body.pointer("/variables/ids"), Some(&json!(["R_other"])));
    assert!(body
        .get("query")
        .and_then(Value::as_str)
        .is_some_and(|q| q.contains("nodes(ids:$ids)")));
    assert!(seen
        .iter()
        .all(|w| !String::from_utf8_lossy(&w.body).contains("createPullRequest")));
    let refusals = rig.refusals(1);
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals.first().map(|r| r.method.as_str()), Some("POST"));
    assert_eq!(refusals.first().map(|r| r.category), Some(SCOPE_CATEGORY));
}

#[test]
fn node_id_of_task_repo_is_looked_up_then_forwarded() {
    let rig = rig(standard(nodes_reply(&["IniZio/moor"])));
    let mut client = rig.client();
    let sent = post("/graphql", &create_pr("R_task"));
    let resp = client.call(&sent);
    assert_eq!(resp.status(), 200);
    assert_eq!(String::from_utf8_lossy(&resp.body), OK_BODY);
    let seen = rig.requests();
    assert_eq!(seen.len(), 2);
    assert!(seen.first().is_some_and(|w| w.is_lookup() && has_token(w)));
    assert!(
        seen.get(1).is_some_and(|w| w.raw == sent),
        "forwarded bytes changed"
    );
    assert!(rig.refusals(0).is_empty());
}

#[test]
fn node_lookup_is_cached_per_filter() {
    let rig = rig(standard(nodes_reply(&["IniZio/moor"])));
    let sent = post("/graphql", &create_pr("R_task"));
    for _ in 0..2 {
        let resp = rig.client().call(&sent);
        assert_eq!(resp.status(), 200);
    }
    assert_eq!(rig.lookups(), 1);
    assert_eq!(rig.requests().len(), 3);
}

#[test]
fn foreign_node_verdict_is_cached_too() {
    let rig = rig(standard(nodes_reply(&["victim/secrets"])));
    let sent = post("/graphql", &create_pr("R_other"));
    for _ in 0..2 {
        assert_refused(&rig.client().call(&sent), 403);
    }
    assert_eq!(rig.lookups(), 1);
    assert_eq!(rig.requests().len(), 1);
}

/// Runs a node-check request against an upstream whose lookup answer is `bad`.
fn failed_lookup_is_refused_and_not_cached(bad: Vec<u8>) {
    let rig = rig(standard(bad));
    let sent = post("/graphql", &create_pr("R_task"));
    for _ in 0..2 {
        assert_refused(&rig.client().call(&sent), 403);
    }
    assert_eq!(rig.lookups(), 2, "a failed lookup must not be cached");
    assert_eq!(
        rig.requests().len(),
        2,
        "the mutation must never go upstream"
    );
    assert_eq!(rig.refusals(2).len(), 2);
}

#[test]
fn lookup_answered_with_server_error_is_refused_and_not_cached() {
    failed_lookup_is_refused_and_not_cached(json_reply(
        "500 Internal Server Error",
        r#"{"message":"boom"}"#,
    ));
}

#[test]
fn lookup_answered_with_garbage_is_refused_and_not_cached() {
    failed_lookup_is_refused_and_not_cached(json_reply("200 OK", "this is not json"));
}

#[test]
fn lookup_answered_with_wrong_node_count_is_refused() {
    failed_lookup_is_refused_and_not_cached(nodes_reply(&["IniZio/moor", "IniZio/moor"]));
}

#[test]
fn lookup_answered_with_null_node_is_refused() {
    let null = json_reply("200 OK", r#"{"data":{"nodes":[null]}}"#);
    let rig = rig(standard(null));
    assert_refused(
        &rig.client().call(&post("/graphql", &create_pr("R_gone"))),
        403,
    );
    assert_eq!(rig.requests().len(), 1);
}

#[test]
fn lookup_with_graphql_errors_is_refused() {
    let errors = json_reply(
        "200 OK",
        r#"{"data":{"nodes":[{"__typename":"Repository","nameWithOwner":"IniZio/moor"}]},"errors":[{"message":"x"}]}"#,
    );
    let rig = rig(standard(errors));
    assert_refused(
        &rig.client().call(&post("/graphql", &create_pr("R_task"))),
        403,
    );
    assert_eq!(rig.requests().len(), 1);
}

#[test]
fn nothing_leaks_into_refusals_debug_output_or_error_body() {
    let marker = "MARKER_zq81";
    let rig = rig(standard(nodes_reply(&[])));
    let before = format!("{:?}", rig.filter);
    let resp = rig
        .client()
        .call(&post("/graphql", &repo_query("victim", "secrets", marker)));
    assert_refused(&resp, 403);
    let refusals = rig.refusals(1);
    assert_eq!(refusals.len(), 1);
    let text = format!("{refusals:?} {before} {:?}", rig.filter);
    assert!(!text.contains(TOKEN), "credential leaked into debug output");
    assert!(
        !text.contains(marker),
        "request body leaked into debug output"
    );
    assert!(!text.contains("gho_"));
    let body = String::from_utf8_lossy(&resp.raw);
    assert!(!body.contains(marker), "request body echoed in the error");
}
