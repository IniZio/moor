//! A rule's `app` lines: sent with the GitHub App's token, as a rule of their own.

use super::*;
use crate::config::parse_repo;

fn with_app(lines: &[&str]) -> ProxyConfig {
    let mut config = ProxyConfig::default();
    config.rules.insert(
        "github-api".to_owned(),
        ToolRule {
            app: lines.iter().map(|l| (*l).to_owned()).collect(),
            ..ToolRule::default()
        },
    );
    config
}

#[test]
fn app_lines_become_a_rule_per_repo_after_theirs_that_injects_the_app_token_for_that_repo() {
    let other = ForgeRepo::new("acme", "gadgets");
    let rules = resolve_rules(
        &with_app(&[
            "POST /repos/{repo}/statuses/*",
            "POST|PATCH /repos/{repo}/issues/*/comments",
        ]),
        &[repo(), other],
        false,
    )
    .expect("resolves");
    let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "claude",
            "github-api",
            "github-api-app",
            "github-api-app-2",
            "github-git",
            "github-uploads"
        ]
    );
    let app = rules
        .iter()
        .find(|r| r.name == "github-api-app")
        .expect("app rule");
    let api = rules
        .iter()
        .find(|r| r.name == "github-api")
        .expect("api rule");
    assert_eq!(app.credential, APP_CREDENTIAL);
    assert_eq!(app.app_repo, Some(repo()));
    assert_eq!(app.hosts, api.hosts);
    assert_eq!(app.inject, api.inject);
    assert!(app.box_env.is_empty() && app.graphql.is_none());
    let lines: Vec<String> = app
        .endpoint_rules
        .iter()
        .map(|e| format!("{} {}", e.method, e.path))
        .collect();
    assert_eq!(
        lines,
        [
            "POST /repos/acme/widgets/statuses/*",
            "POST /repos/acme/widgets/issues/*/comments",
            "PATCH /repos/acme/widgets/issues/*/comments",
        ]
    );
    assert!(
        !api.endpoint_rules
            .iter()
            .any(|e| e.path.ends_with("/statuses/*")),
        "the user's token does not serve them"
    );
}

#[test]
fn a_later_repo_gets_its_own_app_rule_with_its_own_lines() {
    let other = ForgeRepo::new("acme", "gadgets");
    let rules = resolve_rules(
        &with_app(&[
            "POST /repos/{repo}/statuses/*",
            "POST|PATCH /repos/{repo}/issues/*/comments",
        ]),
        &[repo(), other],
        false,
    )
    .expect("resolves");
    let second = rules
        .iter()
        .find(|r| r.name == "github-api-app-2")
        .expect("second repo's app rule");
    assert_eq!(second.app_repo, Some(ForgeRepo::new("acme", "gadgets")));
    let lines: Vec<&str> = second
        .endpoint_rules
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(
        lines,
        [
            "/repos/acme/gadgets/statuses/*",
            "/repos/acme/gadgets/issues/*/comments",
            "/repos/acme/gadgets/issues/*/comments",
        ]
    );
}

#[test]
fn no_app_rule_without_lines_or_without_a_repo() {
    for (config, repos) in [
        (ProxyConfig::default(), vec![repo()]),
        (with_app(&["POST /repos/{repo}/statuses/*"]), vec![]),
    ] {
        let rules = resolve_rules(&config, &repos, true).expect("resolves");
        assert!(
            rules.iter().all(|r| r.credential != APP_CREDENTIAL),
            "{rules:?}"
        );
    }
}

#[test]
fn app_lines_are_refused_off_github_on_any_path_or_beside_the_same_allow_line() {
    let off = err("proxy: {rules: {x: {hosts: [a.com], credential: x, inject: {basic_user: u}, app: ['POST /x']}}}");
    assert!(off.reason.contains("api.github.com"), "{off}");
    let any = err("proxy: {rules: {github-api: {app: ['*']}}}");
    assert!(any.reason.contains("REST path"), "{any}");
    let both = err("proxy: {rules: {github-api: {allow: ['POST /repos/{repo}/statuses/*'], app: ['POST /repos/{repo}/statuses/*']}}}");
    assert!(both.reason.contains("also in allow"), "{both}");
}

#[test]
fn a_repo_config_asks_for_app_lines_under_github_app() {
    let parsed = parse_repo(
        "m.yaml",
        "proxy: {allow: {github-app: [\"POST /repos/{repo}/statuses/*\"]}}",
    );
    assert!(parsed.is_ok(), "{parsed:?}");
}

#[test]
fn no_rule_may_inject_the_app_token_as_its_credential() {
    let e = err(
        "proxy: {rules: {x: {hosts: [a.com], credential: github-app, inject: {basic_user: u}}}}",
    );
    assert_eq!(e.path, "proxy.rules.x.credential");
    assert!(e.reason.contains("app lines only"), "{e}");
}
