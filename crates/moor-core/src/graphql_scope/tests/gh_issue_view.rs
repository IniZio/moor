//! Tests for the queries `gh issue view` and `gh pr view` send, as gh 2.102 writes them.

use serde_json::{json, Value};

use super::{allowed, body, denied, under_box};
use crate::graphql_scope::{judge_linked, RepoRef, Verdict};

const ISSUE_VIEW: &str = "query IssueByNumber($owner: String!, $repo: String!, $number: Int!) { repository(owner: $owner, name: $repo) { hasIssuesEnabled issue: issueOrPullRequest(number: $number) { __typename ...on Issue{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},comments(last: 1) {nodes {author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}}},totalCount},stateReason,issueType{id,name,description,color},parent{id,number,title,url,state,repository{nameWithOwner}},subIssues(first:100){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},subIssuesSummary{total,completed,percentCompleted},blockedBy(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},blocking(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},id} ...on PullRequest{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},comments(last: 1) {nodes {author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}}},totalCount},id} } } }";

const ISSUE_VIEW_COMMENTS: &str = "query IssueByNumber($owner: String!, $repo: String!, $number: Int!) { repository(owner: $owner, name: $repo) { hasIssuesEnabled issue: issueOrPullRequest(number: $number) { __typename ...on Issue{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},stateReason,issueType{id,name,description,color},parent{id,number,title,url,state,repository{nameWithOwner}},subIssues(first:100){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},subIssuesSummary{total,completed,percentCompleted},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},blockedBy(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},blocking(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},id} ...on PullRequest{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},id} } } }";

const ISSUE_PROJECT_ITEMS: &str = r#"query IssueProjectItems($endCursor:String$name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){issue(number: $number){projectItems(first: 100, after: $endCursor){totalCount,nodes{id,project{id,title},status:fieldValueByName(name: "Status"){... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},pageInfo{hasNextPage,endCursor}}}}}"#;

const PR_VIEW: &str = "query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $pr_number) {url,number,title,state,body,author{login,...on User{id,name}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},isDraft,maintainerCanModify,mergeable,additions,deletions,commits{totalCount},baseRefName,headRefName,headRepositoryOwner{id,login,...on User{name}},headRepository{id,name,nameWithOwner},isCrossRepository,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},reviews(first: 100) {nodes {id,author{login},authorAssociation,submittedAt,body,state,commit{oid},reactionGroups{content,users{totalCount}}}pageInfo{hasNextPage,endCursor}totalCount},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},reactionGroups{content,users{totalCount}},createdAt,statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}},id} } }";

const PR_PROJECT_ITEMS: &str = r#"query PullRequestProjectItems($endCursor:String$name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){pullRequest(number: $number){projectItems(first: 100, after: $endCursor){totalCount,nodes{id,project{id,title},status:fieldValueByName(name: "Status"){... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},pageInfo{hasNextPage,endCursor}}}}}"#;

/// Allows `query` for the Box repo and refuses it, for naming the repo, for another one.
fn scoped(query: &str, vars: impl Fn(&str) -> Value) {
    allowed(query, &vars("IniZio"));
    denied(query, &vars("evil"), "not the Box repo");
}

#[test]
fn gh_issue_view_is_scoped_to_the_box_repo() {
    let by_repo = |o: &str| json!({"owner": o, "repo": "moor", "number": 1104});
    scoped(ISSUE_VIEW, by_repo);
    scoped(ISSUE_VIEW_COMMENTS, by_repo);
    scoped(
        ISSUE_PROJECT_ITEMS,
        |o| json!({"owner": o, "name": "moor", "number": 1104, "endCursor": null}),
    );
}

#[test]
fn gh_pr_view_is_scoped_to_the_box_repo() {
    scoped(
        PR_VIEW,
        |o| json!({"owner": o, "repo": "moor", "pr_number": 1}),
    );
    scoped(
        PR_PROJECT_ITEMS,
        |o| json!({"owner": o, "name": "moor", "number": 1, "endCursor": null}),
    );
}

#[test]
fn linked_issues_stay_identity_only() {
    for path in [
        "parent",
        "subIssues(first: 1) { nodes",
        "blockedBy(first: 1) { nodes",
        "blocking(first: 1) { nodes",
    ] {
        let close = if path.contains('{') { " } } }" } else { " } }" };
        for (probe, field) in [
            ("comments(first: 1) { nodes { body } }", "comments"),
            ("body", "body"),
        ] {
            denied(
                &under_box(&format!("issue(number: 1) {{ {path} {{ {probe}{close}")),
                &json!({}),
                &format!("linkedissue.{field}"),
            );
        }
    }
}

#[test]
fn aliases_inside_a_linked_issue_are_refused() {
    for selection in [
        "parent { url: title }",
        "parent { t: state }",
        "subIssues(first: 1) { nodes { title: url repository { nameWithOwner } } }",
        "parent { title repo: repository { nameWithOwner } }",
        "parent { title repository { n: nameWithOwner } }",
    ] {
        denied(
            &under_box(&format!("issue(number: 1) {{ {selection} }}")),
            &json!({}),
            "alias",
        );
    }
    allowed(
        &under_box("issue(number: 1) { p: parent { title: title repository { nameWithOwner } } }"),
        &json!({}),
    );
}

/// A linked issue in `repo`.
fn linked(repo: &str) -> Value {
    json!({"title": "t", "state": "OPEN", "repository": {"nameWithOwner": repo}})
}

#[test]
fn the_gh_issue_view_response_shows_only_box_repo_headlines() {
    let (verdict, paths) = judge_linked(
        &body(
            ISSUE_VIEW,
            &json!({"owner": "IniZio", "repo": "moor", "number": 1}),
        ),
        &RepoRef::new("IniZio", "moor"),
    );
    assert_eq!(verdict, Verdict::Allow);
    let mut resp = json!({"data": {"repository": {"issue": {
        "title": "own", "state": "OPEN",
        "parent": linked("iniZio/MOOR"),
        "subIssues": {"nodes": [linked("IniZio/lib"), linked("victim/secrets")]},
        "blockedBy": {"nodes": [linked("victim/secrets")]},
        "blocking": {"nodes": [{"title": "t", "state": "OPEN"}]},
    }}}});
    paths.blank(
        &mut resp,
        &[
            RepoRef::new("IniZio", "moor"),
            RepoRef::new("IniZio", "lib"),
        ],
    );
    let blank =
        |repo: &str| json!({"title": null, "state": null, "repository": {"nameWithOwner": repo}});
    assert_eq!(
        resp,
        json!({"data": {"repository": {"issue": {
            "title": "own", "state": "OPEN",
            "parent": linked("iniZio/MOOR"),
            "subIssues": {"nodes": [linked("IniZio/lib"), blank("victim/secrets")]},
            "blockedBy": {"nodes": [blank("victim/secrets")]},
            "blocking": {"nodes": [{"title": null, "state": null}]},
        }}}})
    );
}

#[test]
fn a_query_without_linked_issues_needs_no_response_check() {
    let (verdict, paths) = judge_linked(
        &body(
            PR_VIEW,
            &json!({"owner": "IniZio", "repo": "moor", "pr_number": 1}),
        ),
        &RepoRef::new("IniZio", "moor"),
    );
    assert_eq!(verdict, Verdict::Allow);
    assert!(paths.is_empty());
}
