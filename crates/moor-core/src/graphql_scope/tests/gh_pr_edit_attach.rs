//! The lookup `gh pr edit --attach` sends before it uploads, captured from gh 2.102.0.

use serde_json::json;

use super::{allowed, denied};

const BY_NUMBER_ATTACH: &str = "query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) {
    repository(owner: $owner, name: $repo) {
      pullRequest(number: $pr_number) {id,author{login,...on User{id,name}},url,title,body,baseRefName,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn},repository{id,name,nameWithOwner,databaseId,viewerPermission},assignedActors(first: 10) {nodes {...on User {id,login,name,__typename}...on Bot {id,login,__typename}},totalCount},number}
    }
  }";

#[test]
fn gh_pr_edit_attach_lookup_is_scoped_to_the_box_repo() {
    allowed(
        BY_NUMBER_ATTACH,
        &json!({"owner": "IniZio", "repo": "moor", "pr_number": 32}),
    );
    denied(
        BY_NUMBER_ATTACH,
        &json!({"owner": "evil", "repo": "moor", "pr_number": 32}),
        "not the Box repo",
    );
}

#[test]
fn gh_pr_edit_update_mutation_needs_a_node_check() {
    for result in ["__typename", "pullRequest{id}", "clientMutationId"] {
        let query = format!(
            "mutation PullRequestUpdate($input:UpdatePullRequestInput!){{updatePullRequest(input: $input){{{result}}}}}"
        );
        super::needs(
            &query,
            &json!({"input": {"pullRequestId": "PR_kwDOVIbOMs6x", "body": "b"}}),
            &["PR_kwDOVIbOMs6x"],
        );
    }
}
