//! Tests for the node-id lookup helpers.

use moor_core::graphql_scope::{judge, NodeId, RepoRef, Verdict};
use serde_json::{json, Value};

use super::{lookup_body, lookup_request, resolve, NodeCache};

fn repo() -> RepoRef {
    RepoRef::new("IniZio", "moor")
}

/// Node ids obtained through the real scope check.
fn ids(names: &[&str]) -> Vec<NodeId> {
    let mut out = Vec::new();
    for name in names {
        let body = serde_json::to_vec(&json!({
            "query": "query($id: ID!) { node(id: $id) { ... on PullRequest { title } } }",
            "variables": { "id": name },
        }))
        .unwrap();
        let verdict = judge(&body, &repo());
        assert!(matches!(verdict, Verdict::NeedsNodeCheck(_)), "{verdict:?}");
        if let Verdict::NeedsNodeCheck(found) = verdict {
            out.extend(found);
        }
    }
    out
}

fn answer(nodes: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "data": { "nodes": nodes } })).unwrap()
}

#[test]
fn request_carries_authorization_and_length() {
    let ids = ids(&["PR_1"]);
    let req = lookup_request("api.github.com", "Bearer s3cret", &ids);
    let text = String::from_utf8(req).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /graphql HTTP/1.1\r\nHost: api.github.com\r\n"));
    assert!(head.contains("\r\nAuthorization: Bearer s3cret\r\n"));
    assert!(head.contains("\r\nConnection: close"));
    assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", body.len())));
    assert_eq!(body.as_bytes(), lookup_body(&ids));
}

#[test]
fn body_passes_ids_as_variables() {
    let ids = ids(&["PR_1", "I_2"]);
    let value: Value = serde_json::from_slice(&lookup_body(&ids)).unwrap();
    assert_eq!(
        value.pointer("/variables/ids"),
        Some(&json!(["PR_1", "I_2"]))
    );
    let query = value.pointer("/query").and_then(Value::as_str).unwrap();
    assert!(query.contains("nodes(ids:$ids)"));
    assert!(!query.contains("PR_1"));
}

#[test]
fn hostile_id_is_escaped_not_injected() {
    let evil = "x\"}) { viewer { login } } #}";
    let ids = ids(&[evil]);
    let value: Value = serde_json::from_slice(&lookup_body(&ids)).unwrap();
    assert_eq!(value.pointer("/variables/ids"), Some(&json!([evil])));
    assert!(!value
        .pointer("/query")
        .and_then(Value::as_str)
        .unwrap()
        .contains("viewer"));
}

#[test]
fn box_repo_nodes_resolve_true() {
    let ids = ids(&["R", "P", "I"]);
    let body = answer(&json!([
        { "__typename": "Repository", "nameWithOwner": "IniZio/moor" },
        { "__typename": "PullRequest", "repository": { "nameWithOwner": "IniZio/moor" } },
        { "__typename": "Issue", "repository": { "nameWithOwner": "IniZio/moor" } },
    ]));
    assert_eq!(resolve(&body, &ids, &repo()), Some(vec![true, true, true]));
}

#[test]
fn other_null_and_unknown_nodes_resolve_false() {
    let ids = ids(&["A", "B", "C", "D"]);
    let body = answer(&json!([
        { "__typename": "Repository", "nameWithOwner": "IniZio/other" },
        { "__typename": "PullRequest", "repository": { "nameWithOwner": "evil/moor" } },
        null,
        { "__typename": "User", "nameWithOwner": "IniZio/moor" },
    ]));
    assert_eq!(
        resolve(&body, &ids, &repo()),
        Some(vec![false, false, false, false])
    );
}

#[test]
fn names_compare_case_insensitively() {
    let ids = ids(&["R"]);
    let body = answer(&json!([
        { "__typename": "Repository", "nameWithOwner": "inizio/MOOR" },
    ]));
    assert_eq!(resolve(&body, &ids, &repo()), Some(vec![true]));
}

#[test]
fn unusable_bodies_resolve_to_none() {
    let ids = ids(&["A", "B"]);
    let ok = json!([null, null]);
    assert_eq!(resolve(b"not json", &ids, &repo()), None);
    assert_eq!(resolve(b"{}", &ids, &repo()), None);
    let errors =
        serde_json::to_vec(&json!({ "data": { "nodes": ok }, "errors": [{ "message": "x" }] }))
            .unwrap();
    assert_eq!(resolve(&errors, &ids, &repo()), None);
    assert_eq!(resolve(&answer(&json!([null])), &ids, &repo()), None);
    assert_eq!(resolve(&answer(&json!("nodes")), &ids, &repo()), None);
}

#[test]
fn cache_remembers_verdicts() {
    let ids = ids(&["A", "B", "C"]);
    let cache = NodeCache::new();
    let [a, b, c] = ids.as_slice() else {
        return;
    };
    assert_eq!(cache.get(a), None);
    cache.put(a, true);
    cache.put(b, false);
    assert_eq!(cache.get(a), Some(true));
    assert_eq!(cache.get(b), Some(false));
    assert_eq!(cache.get(c), None);
    assert_eq!(format!("{cache:?}"), "NodeCache { entries: 2 }");
}
