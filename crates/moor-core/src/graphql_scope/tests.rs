//! Tests for the GraphQL scope check.

use std::fmt::Write;

use serde_json::{json, Value};

use super::{judge, NodeId, RepoRef, Verdict};

mod extra;

fn repo() -> RepoRef {
    RepoRef::new("IniZio", "moor")
}

fn body(query: &str, vars: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "query": query, "variables": vars })).unwrap()
}

fn verdict(query: &str, vars: &Value) -> Verdict {
    judge(&body(query, vars), &repo())
}

pub(super) fn allowed(query: &str, vars: &Value) {
    assert_eq!(verdict(query, vars), Verdict::Allow, "{query}");
}

pub(super) fn denied(query: &str, vars: &Value, needle: &str) {
    match verdict(query, vars) {
        Verdict::Deny(reason) => assert!(
            reason.as_str().contains(needle),
            "reason `{}` lacks `{needle}`",
            reason.as_str()
        ),
        other => unreachable!("expected deny for {query}, got {other:?}"),
    }
}

pub(super) fn needs(query: &str, vars: &Value, ids: &[&str]) {
    let want: Vec<NodeId> = ids.iter().map(|id| NodeId::new(*id)).collect();
    assert_eq!(
        verdict(query, vars),
        Verdict::NeedsNodeCheck(want),
        "{query}"
    );
}

const PR_BY_NUMBER: &str = r"
query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $pr_number) {
      id number title state body url
      author { login }
      headRepository { nameWithOwner }
      headRepositoryOwner { login }
      commits(last: 1) { totalCount nodes { commit { oid statusCheckRollup { state } } } }
      labels(first: 100) { nodes { name } }
    }
  }
}";

const PR_PROJECT_ITEMS: &str = r"
query PullRequestProjectItems($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      projectItems(first: 100, after: $endCursor) {
        nodes { id project { id title } }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}";

const PR_LIST: &str = r"
query PullRequestList($owner: String!, $repo: String!, $limit: Int!, $state: [PullRequestState!]) {
  repository(owner: $owner, name: $repo) {
    pullRequests(first: $limit, states: $state, orderBy: {field: CREATED_AT, direction: DESC}) {
      totalCount
      nodes { number title headRefName isDraft }
    }
  }
}";

const REPO_INFO: &str = r"
query RepositoryInfo($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    id name owner { login } hasIssuesEnabled description viewerPermission defaultBranchRef { name }
    isFork parent { nameWithOwner }
  }
}";

const PR_FOR_BRANCH: &str = r"
query PullRequestForBranch($owner: String!, $repo: String!, $headRefName: String!, $states: [PullRequestState!]) {
  repository(owner: $owner, name: $repo) {
    pullRequests(headRefName: $headRefName, states: $states, first: 30) {
      nodes { id number title url baseRefName headRefName isCrossRepository }
    }
  }
}";

const REPO_NETWORK: &str = r#"
query RepositoryNetwork {
  repo_000: repository(owner: "IniZio", name: "moor") {
    id name owner { login } viewerPermission defaultBranchRef { name }
  }
  viewer { login }
}"#;

const GH_STACK_FIND: &str = r"
query FindPRForBranch($owner: String!, $name: String!, $head: String!) {
  repository(owner: $owner, name: $name) {
    pullRequests(headRefName: $head, states: [OPEN], first: 1) { nodes { id number url } }
  }
}";

const GH_STACK_BY_NUMBER: &str = r"
query FindPRByNumber($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) { id number state isDraft baseRefName headRefName }
  }
}";

const REPOSITORY_ID: &str = r"
query RepositoryID($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) { id }
}";

fn on_task() -> Value {
    json!({"owner": "IniZio", "repo": "moor", "name": "moor", "pr_number": 139,
           "number": 139, "limit": 2, "state": ["OPEN"], "states": ["OPEN"],
           "headRefName": "next", "head": "next", "endCursor": null})
}

#[test]
fn captured_read_shapes_are_allowed() {
    for query in [
        PR_BY_NUMBER,
        PR_PROJECT_ITEMS,
        PR_LIST,
        REPO_INFO,
        PR_FOR_BRANCH,
        REPO_NETWORK,
        GH_STACK_FIND,
        GH_STACK_BY_NUMBER,
        REPOSITORY_ID,
    ] {
        allowed(query, &on_task());
    }
}

#[test]
fn repo_match_ignores_case() {
    let vars = json!({"owner": "inizio", "name": "MOOR"});
    allowed(REPOSITORY_ID, &vars);
}

#[test]
fn other_repo_by_variable_is_denied() {
    let vars = json!({"owner": "victim", "name": "secrets"});
    denied(REPOSITORY_ID, &vars, "victim/secrets");
    denied(
        REPOSITORY_ID,
        &json!({"owner": "IniZio", "name": "other"}),
        "IniZio/other",
    );
}

#[test]
fn other_repo_by_literal_is_denied() {
    let query = r#"{ repo_000: repository(owner: "IniZio", name: "moor") { id }
                     repo_001: repository(owner: "victim", name: "secrets") { id } }"#;
    denied(query, &json!({}), "victim/secrets");
}

#[test]
fn missing_or_extra_repository_args_are_denied() {
    denied(
        "{ repository(owner: \"IniZio\") { id } }",
        &json!({}),
        "`name`",
    );
    denied(
        "{ repository(owner: \"IniZio\", name: \"moor\", followRenames: false) { id } }",
        &json!({}),
        "followRenames",
    );
    denied(REPOSITORY_ID, &json!({"owner": "IniZio"}), "not provided");
    denied(
        REPOSITORY_ID,
        &json!({"owner": "IniZio", "name": 5}),
        "string",
    );
}

#[test]
fn variable_defaults_are_used_when_unset() {
    let query = r#"query($o: String = "IniZio", $n: String = "moor") {
        repository(owner: $o, name: $n) { id } }"#;
    allowed(query, &json!({}));
    denied(query, &json!({"o": "victim"}), "victim/moor");
}

#[test]
fn viewer_allows_identity_only() {
    allowed("query UserCurrent { viewer { login id } }", &json!({}));
    denied("{ viewer { email } }", &json!({}), "email");
    denied(
        "{ viewer { repositories(first: 5) { nodes { name } } } }",
        &json!({}),
        "repositories",
    );
    denied(
        "{ viewer { login organizations { totalCount } } }",
        &json!({}),
        "organizations",
    );
}

#[test]
fn enumerating_roots_are_denied() {
    for (query, field) in [
        (
            "{ search(query: \"x\", type: REPOSITORY, first: 5) { repositoryCount } }",
            "search",
        ),
        ("{ organization(login: \"o\") { id } }", "organization"),
        ("{ user(login: \"u\") { id } }", "user"),
        (
            "{ repositoryOwner(login: \"u\") { id } }",
            "repositoryOwner",
        ),
        ("{ __schema { types { name } } }", "__schema"),
        ("{ rateLimit { cost } }", "rateLimit"),
    ] {
        denied(query, &json!({}), field);
    }
}

#[test]
fn nested_enumeration_under_repository_is_denied() {
    let owner = r#"{ repository(owner: "IniZio", name: "moor") {
        owner { ... on User { repositories(first: 50) { nodes { name } } } } } }"#;
    denied(owner, &json!({}), "repositories");
    let sibling = r#"{ repository(owner: "IniZio", name: "moor") {
        owner { ... on User { repository(name: "private") { id } } } } }"#;
    denied(sibling, &json!({}), "repository");
    let hop = r#"{ repository(owner: "IniZio", name: "moor") { id viewer { id } } }"#;
    denied(hop, &json!({}), "viewer");
}

#[test]
fn fragment_cannot_hide_a_root_field() {
    let query = r#"
      query { ...Hidden repository(owner: "IniZio", name: "moor") { id } }
      fragment Hidden on Query { organization(login: "victim") { id } }"#;
    denied(query, &json!({}), "organization");
    let inline = r#"query { ... on Query { search(query: "x", type: REPOSITORY, first: 1) { repositoryCount } } }"#;
    denied(inline, &json!({}), "search");
    let nested = r#"
      query { repository(owner: "IniZio", name: "moor") { ...F } }
      fragment F on Repository { owner { ...G } }
      fragment G on RepositoryOwner { repositories(first: 9) { totalCount } }"#;
    denied(nested, &json!({}), "repositories");
}

#[test]
fn fragments_on_allowed_fields_work() {
    let query = r#"
      query { repository(owner: "IniZio", name: "moor") { ...F } }
      fragment F on Repository { id name }"#;
    allowed(query, &json!({}));
}

#[test]
fn bad_fragments_are_denied() {
    denied("{ ...Missing }", &json!({}), "unknown fragment");
    let cyclic = r#"query { repository(owner: "IniZio", name: "moor") { ...A } }
        fragment A on Repository { ...B } fragment B on Repository { ...A }"#;
    denied(cyclic, &json!({}), "cyclic");
    let twice =
        "{ __typename } fragment A on Query { __typename } fragment A on Query { __typename }";
    denied(twice, &json!({}), "fragment");
}

#[test]
fn fragment_bomb_is_bounded() {
    let mut query =
        String::from(r#"query { repository(owner: "IniZio", name: "moor") { ...F0 } }"#);
    for i in 0..12 {
        write!(
            query,
            " fragment F{i} on Repository {{ ...F{n} ...F{n} }}",
            n = i + 1
        )
        .unwrap();
    }
    query.push_str(" fragment F12 on Repository { id }");
    denied(&query, &json!({}), "");
}

#[test]
fn aliases_cannot_evade() {
    let renamed = r#"query { repository: organization(login: "victim") { id } }"#;
    denied(renamed, &json!({}), "organization");
    let clash = r#"query { r: repository(owner: "IniZio", name: "moor") { id }
                           r: repository(owner: "IniZio", name: "moor") { name } }"#;
    denied(clash, &json!({}), "used twice");
    let shadow = r#"query { a: viewer { login } a: organization(login: "v") { id } }"#;
    denied(shadow, &json!({}), "used twice");
    let masked = r#"query { repository(owner: "IniZio", name: "moor") { id: owner { x: repositories { totalCount } } } }"#;
    denied(masked, &json!({}), "repositories");
}

#[test]
fn operation_name_does_not_decide() {
    let raw = serde_json::to_vec(&json!({
        "query": "query PullRequestByNumber { organization(login: \"v\") { id } }",
        "operationName": "PullRequestByNumber",
    }))
    .unwrap();
    assert!(matches!(judge(&raw, &repo()), Verdict::Deny(_)));
}

#[test]
fn node_lookups_need_a_check() {
    needs(
        "query($id: ID!) { node(id: $id) { id } }",
        &json!({"id": "R_1"}),
        &["R_1"],
    );
    needs(
        "{ nodes(ids: [\"A\", \"B\", \"A\"]) { id } }",
        &json!({}),
        &["A", "B"],
    );
    needs(
        "query($ids: [ID!]!) { nodes(ids: $ids) { id } }",
        &json!({"ids": ["X"]}),
        &["X"],
    );
    denied(
        "{ node(id: \"A\") { ... on User { repositories { totalCount } } } }",
        &json!({}),
        "not `User`",
    );
    denied("{ nodes(ids: [1]) { id } }", &json!({}), "strings");
}

const CREATE_PR: &str = r"
mutation PullRequestCreate($input: CreatePullRequestInput!) {
  createPullRequest(input: $input) { pullRequest { id number url } }
}";

#[test]
fn create_pull_request_needs_repository_check() {
    let vars = json!({"input": {"repositoryId": "R_kgDO", "baseRefName": "main",
        "headRefName": "next", "title": "t", "body": "b", "draft": false}});
    needs(CREATE_PR, &vars, &["R_kgDO"]);
    let literal = r#"mutation { createPullRequest(input: {repositoryId: "R_lit", baseRefName: "main",
        headRefName: "h", title: "t"}) { pullRequest { id } } }"#;
    needs(literal, &json!({}), &["R_lit"]);
    let fork = json!({"input": {"repositoryId": "R_a", "headRepositoryId": "R_b"}});
    needs(CREATE_PR, &fork, &["R_a", "R_b"]);
    denied(CREATE_PR, &json!({"input": {"title": "t"}}), "repositoryId");
    denied(
        CREATE_PR,
        &json!({"input": {"repositoryId": 7}}),
        "repositoryId",
    );
    denied(CREATE_PR, &json!({}), "not provided");
}

#[test]
fn pull_request_mutations_need_node_checks() {
    for (name, key) in [
        ("updatePullRequest", "pullRequestId"),
        ("markPullRequestReadyForReview", "pullRequestId"),
        ("disablePullRequestAutoMerge", "pullRequestId"),
        ("requestReviewsByLogin", "pullRequestId"),
        ("replaceActorsForAssignable", "assignableId"),
    ] {
        let query =
            format!("mutation M($input: X!) {{ {name}(input: $input) {{ clientMutationId }} }}");
        needs(&query, &json!({"input": {key: "PR_1"}}), &["PR_1"]);
        denied(&query, &json!({"input": {}}), key);
    }
}

#[test]
fn unknown_mutations_are_denied() {
    for name in [
        "createRepository",
        "deleteRepository",
        "addStar",
        "mergePullRequest",
        "forkRepo",
    ] {
        let query = format!("mutation {{ {name}(input: {{}}) {{ clientMutationId }} }}");
        denied(&query, &json!({}), name);
    }
    denied(
        "query { createPullRequest(input: {repositoryId: \"R\"}) { pullRequest { id } } }",
        &json!({}),
        "createPullRequest",
    );
}

#[test]
fn mutation_extra_arguments_are_denied() {
    let query = r#"mutation { updatePullRequest(input: {pullRequestId: "P"}, other: 1) { clientMutationId } }"#;
    denied(query, &json!({}), "other");
}

#[test]
fn mutation_result_cannot_enumerate() {
    let vars = json!({"input": {"repositoryId": "R"}});
    let query = "mutation($input: X!) { createPullRequest(input: $input) { pullRequest { headRepositoryOwner { repositories { totalCount } } } } }";
    denied(query, &vars, "repositories");
}

#[test]
fn multiple_operations_are_denied() {
    let query = r#"query A { viewer { login } } query B { organization(login: "v") { id } }"#;
    denied(query, &json!({}), "more than one");
    denied(
        "fragment F on Query { __typename }",
        &json!({}),
        "no operation",
    );
}

#[test]
fn subscriptions_are_denied() {
    denied(
        "subscription { viewer { login } }",
        &json!({}),
        "subscriptions",
    );
}

#[test]
fn duplicate_json_keys_are_denied() {
    let top = br#"{"query":"{ viewer { login } }","query":"{ organization(login:\"v\") { id } }"}"#;
    assert!(matches!(judge(top, &repo()), Verdict::Deny(r) if r.as_str().contains("duplicate")));
    let vars = br#"{"query":"query($o:String!){repository(owner:$o,name:\"moor\"){id}}",
        "variables":{"o":"victim","o":"IniZio"}}"#;
    assert!(matches!(judge(vars, &repo()), Verdict::Deny(r) if r.as_str().contains("duplicate")));
}

#[test]
fn malformed_requests_are_denied() {
    for raw in [
        &b"[]"[..],
        b"{}",
        b"not json",
        b"{\"query\": 5}",
        b"{\"query\": \"{ viewer { login } }\", \"extensions\": {}}",
        b"{\"query\": \"{ viewer { login } }\", \"variables\": [1]}",
        b"{\"query\": \"{ viewer { login } }\"} trailing",
        b"[{\"query\": \"{ viewer { login } }\"}]",
    ] {
        assert!(matches!(judge(raw, &repo()), Verdict::Deny(_)), "{raw:?}");
    }
    denied("{ viewer { login }", &json!({}), "malformed");
    denied("{ viewer { login } } !", &json!({}), "malformed");
    denied("{ }", &json!({}), "malformed");
    denied(
        "{ viewer(x: \"unterminated) { login } }",
        &json!({}),
        "malformed",
    );
}

#[test]
fn null_and_missing_variables_are_accepted_when_unused() {
    let raw = serde_json::to_vec(&json!({"query": "{ viewer { login } }"})).unwrap();
    assert_eq!(judge(&raw, &repo()), Verdict::Allow);
    let raw =
        serde_json::to_vec(&json!({"query": "{ viewer { login } }", "variables": null})).unwrap();
    assert_eq!(judge(&raw, &repo()), Verdict::Allow);
}

#[test]
fn oversized_body_is_denied() {
    let pad = " ".repeat(70 * 1024);
    denied(
        &format!("{{ viewer {{ login }} }}{pad}"),
        &json!({}),
        "64 KiB",
    );
}

#[test]
fn deep_nesting_is_denied() {
    let chain = |n: usize| {
        let open = "organization { ".repeat(n);
        under_task(&format!("owner {{ {open}id{} }}", " }".repeat(n)))
    };
    denied(&chain(40), &json!({}), "deep");
    allowed(&chain(3), &json!({}));
    let list = format!(
        "{{ node(id: \"x\") {{ id }} }} query($v: Int = {}1{}) {{ __typename }}",
        "[".repeat(50),
        "]".repeat(50)
    );
    denied(&list, &json!({}), "");
}

#[test]
fn lexing_edge_cases() {
    let query = "# comment\n{ repository(owner: \"Ini\\u005Aio\", name: \"\"\"moor\"\"\") @include(if: true) { id } }";
    allowed(query, &json!({}));
    allowed(
        "{ repository(owner: \"IniZio\", name: \"moor\") { id @skip(if: false) } }",
        &json!({}),
    );
    denied("{ viewer { login } } \u{1F600}", &json!({}), "malformed");
}

pub(super) fn under_task(selection: &str) -> String {
    format!("{{ repository(owner: \"IniZio\", name: \"moor\") {{ {selection} }} }}")
}

#[test]
fn reaching_other_repos_through_the_task_repo_is_denied() {
    for (selection, field) in [
        (
            r#"assignableUsers { nodes { repositoriesContributedTo { nodes { nameWithOwner
               object(expression: "HEAD:README.md") { ... on Blob { text } } } } } }"#,
            "actor.repositoriesContributedTo",
        ),
        (
            "assignableUsers { nodes { issues { nodes { body } } pullRequests { nodes { body } }
               issueComments { nodes { body } } } }",
            "actor.issues",
        ),
        ("parent { issues { nodes { title } } }", "repoid.issues"),
        (
            r#"templateRepository { object(expression: "HEAD:x") { ... on Blob { text } } }"#,
            "repo.templateRepository",
        ),
        (
            "owner { ... on Organization { projectsV2 { nodes { items { nodes {
               content { ... on Issue { title body } } } } } } } }",
            "actor.projectsV2",
        ),
        (
            r#"pullRequest(number: 1) { headRepository { object(expression: "HEAD:secret") {
               ... on Blob { text } } } }"#,
            "repoid.object",
        ),
    ] {
        denied(&under_task(selection), &json!({}), field);
    }
}

#[test]
fn leaf_fields_take_no_selection() {
    denied(&under_task("name { x }"), &json!({}), "takes no selection");
}

#[test]
fn every_link_names_a_known_context() {
    use super::schema::{all, shape, Next};
    for s in all() {
        for (name, next) in s.links {
            let (Next::Ctx(target) | Next::Conn(target)) = next;
            assert!(shape(target).is_some(), "{}.{name} -> {target}", s.name);
        }
    }
}

#[test]
fn connection_plumbing_is_allowed() {
    let sel = "pullRequests(first: 5) { totalCount pageInfo { hasNextPage endCursor }
        edges { cursor node { number author { login } } } nodes { number } }";
    allowed(&under_task(sel), &json!({}));
    denied(
        &under_task("pullRequests { edges { node { number } other } }"),
        &json!({}),
        "other",
    );
}
