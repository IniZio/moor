//! The GraphQL roots and mutations a rule's `POST /graphql` entry lists.

use super::{err, repo, user};
use crate::config::proxy::{default_graphql, resolve_rules, ResolvedRule};
use crate::graphql_scope::ROOTS;

#[test]
fn the_defaults_list_the_roots_and_mutations_moor_allowed_before() {
    let scope = default_graphql();
    assert_eq!(scope.query, ROOTS);
    let mutations: Vec<(&str, Vec<&str>)> = scope
        .mutation
        .iter()
        .map(|m| (m.name.as_str(), m.ids.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        mutations,
        [
            (
                "createPullRequest",
                vec!["repositoryId", "headRepositoryId"]
            ),
            ("updatePullRequest", vec!["pullRequestId"]),
            ("markPullRequestReadyForReview", vec!["pullRequestId"]),
            ("disablePullRequestAutoMerge", vec!["pullRequestId"]),
            ("requestReviewsByLogin", vec!["pullRequestId"]),
            ("replaceActorsForAssignable", vec!["assignableId"]),
            ("convertPullRequestToDraft", vec!["pullRequestId"]),
        ]
    );
}

#[test]
fn the_github_api_rule_carries_its_scope_into_the_spec() {
    let rules = resolve_rules(&user("").proxy, &[repo()], false).unwrap();
    let gh = rules
        .iter()
        .find(|r| r.hosts == ["api.github.com"])
        .unwrap();
    assert_eq!(gh.graphql, Some(default_graphql()));
    assert!(rules
        .iter()
        .filter(|r| r.hosts != ["api.github.com"])
        .all(|r| r.graphql.is_none()));
}

#[test]
fn a_user_rule_extending_github_api_may_replace_the_scope() {
    let cfg = user(
        "proxy:\n  rules:\n    github-api:\n      allow:\n        - 'POST /graphql':\n            query: [viewer]\n            mutation: ['mergePullRequest(pullRequestId)']\n",
    );
    let rules = resolve_rules(&cfg.proxy, &[repo()], false).unwrap();
    let scope = rules.iter().find_map(|r| r.graphql.clone()).unwrap();
    assert_eq!(scope.query, ["viewer"]);
    assert!(scope.mutation("mergePullRequest").is_some());
}

#[test]
fn a_scope_belongs_on_api_github_com_only() {
    let e = err(
        "proxy:\n  rules:\n    gitlab:\n      hosts: [gitlab.com]\n      credential: gitlab\n      inject: {form: 'Bearer {secret}'}\n      allow:\n        - 'POST /graphql': {query: [viewer]}\n",
    );
    assert!(e.to_string().contains("api.github.com only"), "{e}");
}

#[test]
fn a_github_graphql_line_must_list_what_it_lets_through() {
    let e = err(
        "proxy:\n  rules:\n    gh2:\n      hosts: [api.github.com]\n      credential: github\n      inject: {form: 'token {secret}'}\n      allow: ['POST /graphql']\n",
    );
    assert!(e.to_string().contains("list the query roots"), "{e}");
}

#[test]
fn only_post_graphql_takes_lists() {
    let e = err(
        "proxy:\n  rules:\n    gh2:\n      hosts: [api.github.com]\n      credential: github\n      inject: {form: 'token {secret}'}\n      allow:\n        - 'GET /graphql': {query: [viewer]}\n",
    );
    assert!(e.to_string().contains("only \"POST /graphql\""), "{e}");
}

#[test]
fn a_spec_recorded_before_rules_carried_a_scope_reads_as_none() {
    let old = r#"{"name":"github-api","hosts":["api.github.com"],"credential":"github",
        "inject":{"Header":{"header":"Authorization","form":"token {secret}"}},
        "endpoint_rules":[],"box_env":{}}"#;
    let rule: ResolvedRule = serde_json::from_str(old).unwrap();
    assert_eq!(rule.graphql, None);
}
