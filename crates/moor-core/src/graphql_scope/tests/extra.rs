//! Tests for search, schema probes, node results, merging and the narrowed contexts.

use serde_json::json;

use super::{allowed, denied, needs, under_box};

const STATUS: &str = r"
query PullRequestStatus($q: String!) {
  viewer { login }
  search(query: $q, type: ISSUE, first: 10) {
    issueCount
    nodes { ... on PullRequest { number title url state headRefName
      reactionGroups { content users { totalCount } }
      commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(last: 10) {
        checkRunCount checkRunCountsByState { state count } } } } } } } }
  }
}";

#[test]
fn search_is_confined_to_the_box_repo() {
    let ok = json!({"q": "repo:IniZio/moor state:open is:pr author:@me"});
    allowed(STATUS, &ok);
    allowed(STATUS, &json!({"q": "REPO:inizio/MOOR is:pr label:wip"}));
    for (q, needle) in [
        ("repo:victim/secrets is:pr", "victim/secrets"),
        ("repo:IniZio/moor repo:victim/secrets", "victim/secrets"),
        ("repo:IniZio/moor repo:IniZio/moor", "repo:IniZio/moor"),
        ("org:victim is:pr", "org:victim"),
        ("repo:IniZio/moor user:victim", "user:victim"),
        ("repo:IniZio/moor owner:victim", "owner:victim"),
        ("repo:IniZio/moor is:public", "is:public"),
        ("repo:IniZio/moor -repo:other/x", "-repo:other/x"),
        ("repo:IniZio/moor OR org:victim", "OR"),
        ("is:pr author:@me", "needs `repo:"),
        ("repo:IniZio/moor (org:victim)", "(org:victim)"),
        (
            "repo:IniZio/moor +repo:victim/secrets",
            "+repo:victim/secrets",
        ),
        (
            "repos:victim/secrets repo:IniZio/moor",
            "repos:victim/secrets",
        ),
        (
            "~repo:victim/secrets repo:IniZio/moor",
            "~repo:victim/secrets",
        ),
        ("repo:IniZio/moor -label:wip", "-label:wip"),
        ("repo:IniZio/moor in:readme", "in:readme"),
        (
            "repo:IniZio/moor author:a\u{a0}org:victim",
            "author:a\u{a0}org:victim",
        ),
        ("repo:IniZio/moor author:a;b", "author:a;b"),
        ("repo:IniZio/moor state:all", "state:all"),
        ("repo:IniZio/moor repo:", "repo:"),
    ] {
        denied(STATUS, &json!({ "q": q }), needle);
    }
}

#[test]
fn search_shape_is_checked() {
    let literal = r#"{ search(query: "repo:IniZio/moor", type: ISSUE, first: 1) { issueCount } }"#;
    allowed(literal, &json!({}));
    let repos =
        r#"{ search(query: "repo:IniZio/moor", type: REPOSITORY, first: 1) { repositoryCount } }"#;
    denied(repos, &json!({}), "type: ISSUE");
    let bad =
        r#"{ search(query: "repo:IniZio/moor", type: ISSUE, first: 1, x: 1) { issueCount } }"#;
    denied(bad, &json!({}), "x");
    let nested = r#"{ search(query: "repo:IniZio/moor", type: ISSUE, first: 1) {
        nodes { ... on PullRequest { author { ... on User { repositories { totalCount } } } } } } }"#;
    denied(nested, &json!({}), "repositories");
}

#[test]
fn schema_probe_is_limited_to_field_names() {
    allowed(
        r#"{ __type(name: "PullRequest") { fields { name } } }"#,
        &json!({}),
    );
    denied(
        r#"{ __type(name: "PullRequest") { fields { name type { name } } } }"#,
        &json!({}),
        "type",
    );
    denied(
        r#"{ __type(name: "PullRequest") { description } }"#,
        &json!({}),
        "description",
    );
}

#[test]
fn node_results_are_read_as_pull_requests() {
    let q = "query($id: ID!) { node(id: $id) { ... on PullRequest { reviews(first: 5) {
        nodes { state author { login } } } comments(first: 5) { nodes { body } } } } }";
    needs(q, &json!({"id": "PR_1"}), &["PR_1"]);
    denied(
        "{ node(id: \"X\") { ... on Repository { name } } }",
        &json!({}),
        "not `Repository`",
    );
    denied(
        "{ node(id: \"X\") { ... on User { login } } }",
        &json!({}),
        "not `User`",
    );
    denied(
        "{ nodes(ids: [\"X\"]) { ...F } } fragment F on Issue { id }",
        &json!({}),
        "not `Issue`",
    );
    denied(
        "{ node(id: \"X\") { ... on PullRequest { repository { name } } } }",
        &json!({}),
        "pr.repository",
    );
    denied("{ node(id: \"X\") { ... on PullRequest { headRepository { object(expression: \"a\") { id } } } } }", &json!({}), "object");
}

#[test]
fn merging_is_refused() {
    for name in ["mergePullRequest", "enablePullRequestAutoMerge"] {
        let q = format!(
            "mutation {{ {name}(input: {{pullRequestId: \"P\"}}) {{ clientMutationId }} }}"
        );
        denied(&q, &json!({}), "merging is not allowed");
    }
    let draft =
        "mutation($i: X!) { convertPullRequestToDraft(input: $i) { pullRequest { id isDraft } } }";
    needs(draft, &json!({"i": {"pullRequestId": "P"}}), &["P"]);
}

#[test]
fn other_repo_pull_requests_stay_hidden() {
    let refs = "refs(first: 5, refPrefix: \"refs/heads/\") { nodes { name associatedPullRequests { nodes { number url
        baseRepository { nameWithOwner } } } } }";
    allowed(&under_box(refs), &json!({}));
    for (sel, field) in [
        (
            "defaultBranchRef { associatedPullRequests { nodes { title } } }",
            "prref.title",
        ),
        (
            "defaultBranchRef { associatedPullRequests { nodes { body } } }",
            "prref.body",
        ),
        (
            "defaultBranchRef { associatedPullRequests { nodes { comments { totalCount } } } }",
            "prref.comments",
        ),
        (
            "pullRequest(number: 1) { closingIssuesReferences { nodes { title } } }",
            "issue.title",
        ),
        (
            "pullRequest(number: 1) { closingIssuesReferences { nodes { state } } }",
            "issue.state",
        ),
    ] {
        denied(&under_box(sel), &json!({}), field);
    }
    allowed(
        &under_box("pullRequest(number: 1) { closingIssuesReferences { nodes { id number url repository { name owner { login } } } } }"),
        &json!({}),
    );
}

#[test]
fn gh_breakage_fields_are_allowed() {
    for sel in [
        "mergeQueue { id }",
        "pullRequest(number: 1) { mergeQueueEntry { id } reactionGroups { content users { totalCount } } }",
        "pullRequest(number: 1) { autoMergeRequest { authorEmail enabledBy { login } } }",
        "pullRequest(number: 1) { comments(first: 1) { nodes { reactionGroups { content } } } }",
        "pullRequest(number: 1) { reviews(first: 1) { nodes { reactionGroups { content } } } }",
        "pullRequest(number: 1) { reviewRequests(first: 5) { nodes { requestedReviewer { ... on Team { slug organization { login } } ... on User { databaseId login } } } } }",
        "pullRequest(number: 1) { projectItems(first: 5) { nodes { id fieldValueByName(name: \"Status\") { ... on ProjectV2ItemFieldSingleSelectValue { optionId name } } } } }",
        "defaultBranchRef { rules(first: 5) { nodes { type } } }",
    ] {
        allowed(&under_box(sel), &json!({}));
    }
}

#[test]
fn gh_list_style_searches_are_allowed() {
    for q in [
        "repo:IniZio/moor is:pr state:open author:@me assignee:octocat label:bug base:main head:feat/x draft:false",
        "repo:IniZio/moor is:pr is:merged sort:updated-desc mentions:octocat milestone:v1.0 login-bug",
        "repo:IniZio/moor type:pr review:approved status:success reviewed-by:@me involves:a",
    ] {
        allowed(STATUS, &json!({ "q": q }));
    }
}

#[test]
fn nested_type_conditions_under_node_are_denied() {
    let inline = "{ node(id: \"X\") { ... on PullRequest { ... on Issue { title } } } }";
    denied(inline, &json!({}), "not `Issue`");
    let spread =
        "{ node(id: \"X\") { ...F } } fragment F on PullRequest { ... on Repository { url } }";
    denied(spread, &json!({}), "not `Repository`");
    let deep = "{ node(id: \"X\") { ... on PullRequest { reviews { nodes { ... on Repository { owner { id } } } } } } }";
    denied(deep, &json!({}), "review.owner");
}

const PR_STATUS: &str = r"
fragment pr on PullRequest { number title state url headRefName isDraft
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(last: 100) {
    totalCount checkRunCount statusContextCount
    checkRunCountsByState { state count } statusContextCountsByState { state count }
    nodes { ... on CheckRun { name status conclusion } ... on StatusContext { context state } }
    pageInfo { hasNextPage endCursor } edges { cursor node { ... on CheckRun { name } } } } } } } } }
query PullRequestStatus($q: String!, $headRefName: String!, $owner: String!, $repo: String!) {
  repository(owner: $owner, name: $repo) {
    pullRequests(headRefName: $headRefName, first: 1) { nodes { ...pr } }
  }
  viewerCreated: search(query: $q, type: ISSUE, first: 10) { totalCount: issueCount edges { node { ...pr } } }
}";

#[test]
fn gh_pr_status_shape_is_allowed() {
    let vars = json!({"q": "repo:IniZio/moor state:open is:pr author:octocat",
        "headRefName": "next", "owner": "IniZio", "repo": "moor"});
    allowed(PR_STATUS, &vars);
    let wrong = "{ repository(owner: \"IniZio\", name: \"moor\") { pullRequest(number: 1) { statusCheckRollup { checkRunCount } } } }";
    denied(wrong, &json!({}), "rollup.checkRunCount");
}

#[test]
fn gh_node_paging_queries_are_allowed() {
    let checks = r"query PullRequestStatusChecks($id: ID!, $endCursor: String) {
      node(id: $id) { ...on PullRequest { statusCheckRollup: commits(last: 1) { nodes { commit {
        statusCheckRollup { contexts(first: 100, after: $endCursor) {
          nodes { __typename
            ...on StatusContext { context state targetUrl createdAt description }
            ...on CheckRun { name status conclusion startedAt completedAt detailsUrl
              checkSuite { workflowRun { event workflow { name } } } } }
          pageInfo { hasNextPage endCursor } } } } } } } } }";
    needs(checks, &json!({"id": "PR_1"}), &["PR_1"]);
    let comments = r"query CommentsForPullRequest($id: ID!, $endCursor: String) {
      node(id: $id) { ...on PullRequest { comments(first: 100, after: $endCursor) {
        nodes { id body author { login ...on User { id name } } reactionGroups { content users { totalCount } } }
        pageInfo { hasNextPage endCursor } } } } }";
    needs(comments, &json!({"id": "PR_1"}), &["PR_1"]);
    let reviewers = r"query($id: ID!) { node(id: $id) { ...on PullRequest { reviewRequests(first: 10) {
        nodes { requestedReviewer { ...on Team { slug name } ...on User { login } } } } } } }";
    needs(reviewers, &json!({"id": "PR_1"}), &["PR_1"]);
}
