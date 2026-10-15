//! Which GraphQL requests reach the upstream, and what the upstream sees.

use super::*;

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
fn other_repo_query_is_forwarded_without_credential() {
    let rig = rig(standard(nodes_reply(&[])));
    let mut client = rig.client();
    let sent = post("/graphql", &repo_query("victim", "secrets", "Q"));
    let resp = client.call(&sent);
    assert_eq!(resp.status(), 200);
    let seen = rig.requests();
    assert_eq!(seen.len(), 1);
    let w = seen.first().unwrap();
    assert_anonymous(w);
    assert_eq!(w.body, sent.split_at(sent.len() - w.body.len()).1);
    assert!(rig.refusals(0).is_empty());
    assert!(!client.closed(), "forwarding keeps the connection");
}

#[test]
fn node_id_of_another_repo_is_forwarded_anonymously_after_one_lookup() {
    let rig = rig(standard(nodes_reply(&["victim/secrets"])));
    let mut client = rig.client();
    let resp = client.call(&post("/graphql", &create_pr("R_other")));
    assert_eq!(resp.status(), 200);
    let seen = rig.requests();
    assert_eq!(seen.len(), 2);
    let lookup = seen.first().expect("lookup");
    assert!(lookup.is_lookup() && has_token(lookup));
    let body: Value = serde_json::from_slice(&lookup.body).unwrap();
    assert_eq!(body.pointer("/variables/ids"), Some(&json!(["R_other"])));
    assert!(body
        .get("query")
        .and_then(Value::as_str)
        .is_some_and(|q| q.contains("nodes(ids:$ids)")));
    let sent = seen.get(1).expect("forwarded");
    assert!(!sent.is_lookup());
    assert_anonymous(sent);
    assert!(rig.refusals(0).is_empty());
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
        assert_eq!(rig.client().call(&sent).status(), 200);
    }
    assert_eq!(rig.lookups(), 1);
    let seen = rig.requests();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().filter(|w| !w.is_lookup()).all(|w| {
        assert_anonymous(w);
        true
    }));
}

/// Runs a node-check request against an upstream whose lookup answer is `bad`.
fn failed_lookup_goes_anonymous_and_is_not_cached(bad: Vec<u8>) {
    let rig = rig(standard(bad));
    let sent = post("/graphql", &create_pr("R_task"));
    for _ in 0..2 {
        assert_eq!(rig.client().call(&sent).status(), 200);
    }
    assert_eq!(rig.lookups(), 2, "a failed lookup must not be cached");
    let seen = rig.requests();
    assert_eq!(seen.len(), 4);
    for w in seen.iter().filter(|w| !w.is_lookup()) {
        assert_anonymous(w);
    }
}

#[test]
fn lookup_answered_with_server_error_goes_anonymous() {
    failed_lookup_goes_anonymous_and_is_not_cached(json_reply(
        "500 Internal Server Error",
        r#"{"message":"boom"}"#,
    ));
}

#[test]
fn lookup_answered_with_garbage_goes_anonymous() {
    failed_lookup_goes_anonymous_and_is_not_cached(json_reply("200 OK", "this is not json"));
}

#[test]
fn lookup_answered_with_wrong_node_count_goes_anonymous() {
    failed_lookup_goes_anonymous_and_is_not_cached(nodes_reply(&["IniZio/moor", "IniZio/moor"]));
}

#[test]
fn lookup_answered_with_null_node_goes_anonymous() {
    let null = json_reply("200 OK", r#"{"data":{"nodes":[null]}}"#);
    let rig = rig(standard(null));
    let resp = rig.client().call(&post("/graphql", &create_pr("R_gone")));
    assert_eq!(resp.status(), 200);
    let seen = rig.requests();
    assert_eq!(seen.len(), 2);
    assert_anonymous(seen.get(1).unwrap());
}

#[test]
fn lookup_with_graphql_errors_goes_anonymous() {
    let errors = json_reply(
        "200 OK",
        r#"{"data":{"nodes":[{"__typename":"Repository","nameWithOwner":"IniZio/moor"}]},"errors":[{"message":"x"}]}"#,
    );
    let rig = rig(standard(errors));
    let resp = rig.client().call(&post("/graphql", &create_pr("R_task")));
    assert_eq!(resp.status(), 200);
    let seen = rig.requests();
    assert_eq!(seen.len(), 2);
    assert_anonymous(seen.get(1).unwrap());
}

#[test]
fn unparsable_body_goes_anonymous() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let resp = rig.client().call(&post("/graphql", "not json at all"));
    assert_eq!(resp.status(), 200);
    assert_anonymous(rig.requests().first().unwrap());
}

#[test]
fn error_bodies_and_debug_output_carry_no_secrets() {
    let marker = "MARKER_zq81";
    let rig = rig(standard(nodes_reply(&[])));
    let before = format!("{:?}", rig.filter);
    let resp = rig
        .client()
        .call(&post("/graphql", &repo_query("victim", "secrets", marker)));
    assert_eq!(resp.status(), 200);
    let text = format!("{before} {:?}", rig.filter);
    assert!(!text.contains(TOKEN), "credential leaked into debug output");
    assert!(!text.contains(marker));
    assert!(!text.contains("gho_"));
}
