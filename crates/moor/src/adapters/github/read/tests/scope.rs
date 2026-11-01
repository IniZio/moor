//! The read a controller makes at Box start, judged by the GraphQL scope filter of the Box it runs in.

use moor_core::graphql_scope::{judge, RepoRef, Verdict};
use serde_json::json;

use crate::adapters::github::read::QUERY;

fn verdict(base: Option<&str>) -> Verdict {
    let body = json!({
        "query": QUERY,
        "variables": {
            "owner": "o",
            "name": "n",
            "branch": "refs/heads/moor/t",
            "base": base.unwrap_or_default(),
            "hasBase": base.is_some(),
        },
    });
    judge(body.to_string().as_bytes(), &RepoRef::new("o", "n"))
}

#[test]
fn read_of_the_box_repo_passes_the_scope_filter() {
    for base in [
        None,
        Some("refs/heads/main"),
        Some("0123456789abcdef0123456789abcdef01234567"),
    ] {
        assert_eq!(verdict(base), Verdict::Allow, "{base:?}");
    }
}

#[test]
fn read_of_another_repo_is_not_allowed() {
    let body = json!({
        "query": QUERY,
        "variables": {"owner": "x", "name": "y", "branch": "refs/heads/b", "base": "", "hasBase": false},
    });
    assert_ne!(
        judge(body.to_string().as_bytes(), &RepoRef::new("o", "n")),
        Verdict::Allow
    );
}
