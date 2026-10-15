//! Tests for the repository, pull-request and issue queries gh sends.

use serde_json::{json, Value};

use super::{allowed, denied, under_task};

const FRAG_OLD: &str = "fragment repo on Repository { id name owner { login } hasIssuesEnabled description hasWikiEnabled viewerPermission defaultBranchRef { name } }";
const FRAG_NEW: &str = "fragment repo on Repository { id databaseId name owner { login } sshUrl hasIssuesEnabled description hasWikiEnabled viewerPermission defaultBranchRef { name } }";
const INFO: &str = " query RepositoryInfo($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { ...repo parent { ...repo } mergeCommitAllowed rebaseMergeAllowed squashMergeAllowed } }";

const BY_NUMBER: &str = "query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $pr_number) {id,author{login,...on User{id,name}},url,title,body,baseRefName,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn},assignedActors(first: 10) {nodes {...on User {id,login,name,__typename}...on Bot {id,login,__typename}},totalCount},number} } }";

const VIEW: &str = "query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $pr_number) {url,number,title,state,body,author{login,...on User{id,name}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},isDraft,maintainerCanModify,mergeable,additions,deletions,commits{totalCount},baseRefName,headRefName,headRepositoryOwner{id,login,...on User{name}},headRepository{id,name},isCrossRepository,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login},...on Team{organization{login}name,slug}}}},assignees(first:100){nodes{id,login,name},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},projectCards(first:100){nodes{project{name}column{name}},totalCount},milestone{number,title,description,dueOn},reactionGroups{content,users{totalCount}},createdAt} } }";

const STATUS: &str = "fragment pr on PullRequest {number,title,state,url,isDraft,isCrossRepository,headRefName,headRepositoryOwner{id,login,...on User{name}},mergeStateStatus,baseRef{branchProtectionRule{requiresStrictStatusChecks,requiredApprovingReviewCount}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts {checkRunCount,checkRunCountsByState {state,count},statusContextCount,statusContextCountsByState {state,count}}}}}}}
fragment prWithReviews on PullRequest {...pr,reviewDecision,latestReviews(first: 100) {nodes {author{login},authorAssociation,submittedAt,body,state}}}
query PullRequestStatus($owner: String!, $repo: String!, $headRefName: String!, $per_page: Int = 10) { repository(owner: $owner, name: $repo) { defaultBranchRef { name } pullRequests(headRefName: $headRefName, first: $per_page, orderBy: { field: CREATED_AT, direction: DESC }) { totalCount edges { node { ...prWithReviews } } } } }";

const ISSUES: &str = "fragment issue on Issue {number,title,url,state,updatedAt,labels(first:100){nodes{id,name,description,color},totalCount},stateReason} query IssueList($owner: String!, $repo: String!, $limit: Int, $endCursor: String, $states: [IssueState!] = OPEN, $assignee: String, $author: String, $mention: String) { repository(owner: $owner, name: $repo) { hasIssuesEnabled issues(first: $limit, after: $endCursor, orderBy: {field: CREATED_AT, direction: DESC}, states: $states, filterBy: {assignee: $assignee, createdBy: $author, mentioned: $mention}) { totalCount nodes { ...issue } pageInfo { hasNextPage endCursor } } } }";

fn network(owner: &str) -> String {
    format!(
        r#"fragment repo on Repository {{ id databaseId name owner {{ login }} viewerPermission defaultBranchRef {{ name }} isPrivate }} query RepositoryNetwork {{ viewer {{ login }} repo_000: repository(owner: "{owner}", name: "moor") {{ ...repo parent {{ ...repo }} }} }}"#
    )
}

fn both(query: &str, vars: impl Fn(&str) -> Value) {
    allowed(query, &vars("IniZio"));
    denied(query, &vars("evil"), "");
}

#[test]
fn gh_repository_queries_are_scoped_to_the_task_repo() {
    let by_name = |o: &str| json!({"owner": o, "name": "moor"});
    both(&format!("{FRAG_OLD}{INFO}"), by_name);
    both(&format!("{FRAG_NEW}{INFO}"), by_name);
    allowed(&network("IniZio"), &json!({}));
    denied(&network("evil"), &json!({}), "");
}

#[test]
fn gh_pr_and_issue_queries_are_scoped_to_the_task_repo() {
    let by_repo = |o: &str| json!({"owner": o, "repo": "moor", "pr_number": 1});
    both(BY_NUMBER, by_repo);
    both(
        STATUS,
        |o| json!({"owner": o, "repo": "moor", "headRefName": "feat"}),
    );
    both(
        ISSUES,
        |o| json!({"owner": o, "repo": "moor", "states": ["OPEN"], "limit": 30}),
    );
}

#[test]
fn gh_pr_view_with_classic_project_cards_is_scoped_to_the_task_repo() {
    both(
        VIEW,
        |o| json!({"owner": o, "repo": "moor", "pr_number": 1}),
    );
}

#[test]
fn classic_project_card_content_stays_denied() {
    denied(
        &under_task(
            "pullRequest(number: 1) { projectCards(first: 1) { nodes { project { columns(first: 1) { nodes { cards(first: 1) { nodes { note } } } } } } } }",
        ),
        &json!({}),
        "",
    );
}

#[test]
fn parent_content_stays_denied() {
    denied(
        &under_task("parent { issues { nodes { title } } }"),
        &json!({}),
        "parent.issues",
    );
    denied(
        &under_task("parent { defaultBranchRef { target { oid } } }"),
        &json!({}),
        "refname.target",
    );
    denied(
        &under_task("parent { pullRequests { nodes { number } } }"),
        &json!({}),
        "parent.pullRequests",
    );
}
