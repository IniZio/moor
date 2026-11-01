//! Files and tags of the Box repo's own commits, and the same fields reached from elsewhere.

use serde_json::json;

use super::{allowed, denied};

const ENTRY: &str = "{ object { __typename ... on Blob { byteSize isBinary text } } }";

#[test]
fn box_repo_commit_files_are_allowed() {
    let query = format!(
        r#"{{ repository(owner: "IniZio", name: "moor") {{
          defaultBranchRef {{ target {{ oid ... on Commit {{ f: file(path: ".moor.yaml") {ENTRY} }} }} }}
          ref(qualifiedName: "refs/heads/main") {{ target {{ ... on Commit {{ file(path: "a") {ENTRY} }} }} }}
          object(expression: "v1") {{ __typename oid ... on Tag {{ target {{ ... on Commit {{ file(path: "a") {ENTRY} }} }} }} }}
        }} }}"#
    );
    allowed(&query, &json!({}));
}

#[test]
fn viewer_profile_name_and_database_id_are_allowed() {
    allowed("{ viewer { login databaseId name } }", &json!({}));
}

#[test]
fn files_of_a_pull_request_head_are_denied() {
    let head = format!(
        r#"{{ repository(owner: "IniZio", name: "moor") {{ pullRequest(number: 1) {{
          headRef {{ target {{ ... on Commit {{ file(path: "a") {ENTRY} }} }} }} }} }} }}"#
    );
    denied(&head, &json!({}), "file");
    let node = format!(
        r#"{{ node(id: "PR_x") {{ ... on PullRequest {{
          headRef {{ target {{ ... on Commit {{ file(path: "a") {ENTRY} }} }} }} }} }} }}"#
    );
    denied(&node, &json!({}), "file");
    let tag = r#"{ node(id: "PR_x") { ... on PullRequest { headRef { target { ... on Tag { target { oid } } } } } } }"#;
    denied(tag, &json!({}), "target");
}

#[test]
fn files_of_another_repo_are_denied() {
    let query = format!(
        r#"{{ repository(owner: "x", name: "y") {{
          defaultBranchRef {{ target {{ ... on Commit {{ file(path: "a") {ENTRY} }} }} }} }} }}"#
    );
    denied(&query, &json!({}), "repository");
}
