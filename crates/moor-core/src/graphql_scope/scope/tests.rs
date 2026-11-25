//! Tests for the scope as the rules text writes it.

use serde_json::json;

use super::Scope;
use crate::graphql_scope::{judge, RepoRef, Verdict};

fn parse(value: serde_json::Value) -> Result<Scope, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

#[test]
fn a_root_with_no_check_in_moor_is_refused() {
    let err = parse(json!({"query": ["repository", "organization"]})).unwrap_err();
    assert!(err.contains("\"organization\" has no check"), "{err}");
}

#[test]
fn a_mutation_must_name_its_id_keys() {
    for bad in [
        "addStar",
        "addStar()",
        "addStar(starrableId",
        "add Star(x)",
        "addStar(a b)",
    ] {
        let err = parse(json!({"mutation": [bad]})).unwrap_err();
        assert!(err.contains("mutationName(idKey"), "{bad}: {err}");
    }
}

#[test]
fn the_text_round_trips() {
    let text = json!({
        "query": ["viewer"],
        "mutation": ["createPullRequest(repositoryId, headRepositoryId)"]
    });
    let scope = parse(text.clone()).unwrap();
    assert_eq!(
        scope.mutation("createPullRequest"),
        Some(&["repositoryId".to_owned(), "headRepositoryId".to_owned()][..])
    );
    assert_eq!(serde_json::to_value(&scope).unwrap(), text);
}

#[test]
fn a_root_left_out_of_the_scope_is_denied() {
    let only_viewer = parse(json!({"query": ["viewer"]})).unwrap();
    let repo = RepoRef::new("o", "n");
    let body = |q: &str| serde_json::to_vec(&json!({ "query": q })).unwrap();
    assert_eq!(
        judge(&body("{ viewer { login } }"), &repo, &only_viewer),
        Verdict::Allow
    );
    let read = body("{ repository(owner: \"o\", name: \"n\") { name } }");
    assert!(matches!(
        judge(&read, &repo, &only_viewer),
        Verdict::Deny(r) if r.as_str().contains("repository")
    ));
}
