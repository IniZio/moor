//! Tests for proxy rule parsing, validation and resolution.

use super::*;
use crate::config::{parse_user, UserConfig};

fn err(doc: &str) -> ConfigError {
    parse_user("u.yaml", doc).expect_err(doc)
}

fn user(doc: &str) -> UserConfig {
    let r = parse_user("u.yaml", doc);
    assert!(r.is_ok(), "{doc}: {r:?}");
    r.unwrap_or_default()
}

const GOOD: &str = "
proxy:
  rules:
    gitlab:
      hosts: [gitlab.com]
      credential: gitlab
      inject: {header: PRIVATE-TOKEN, form: '{secret}'}
      allow: ['GET|POST /api/v4/projects/{repo:urlenc}/merge_requests/**']
      box_env: {GL_REPO: '{repo}'}
";

fn repo() -> (&'static str, &'static str) {
    ("acme", "widgets")
}

#[test]
fn default_rule_hosts_and_credential_cannot_change() {
    let e = err("proxy:\n  rules:\n    github-api:\n      hosts: [evil.example.com]\n");
    assert_eq!(e.path, "proxy.rules.github-api.hosts");
    assert_eq!(
        e.reason,
        "cannot change hosts of the default github-api rule; use a new rule name"
    );
    let e = err("proxy:\n  rules:\n    claude:\n      credential: OTHER\n");
    assert_eq!(e.path, "proxy.rules.claude.credential");
}

#[test]
fn default_rule_star_allow_rejected_at_user_index() {
    let e = err("proxy:\n  rules:\n    github-api:\n      allow: ['*']\n");
    assert_eq!(e.path, "proxy.rules.github-api.allow[0]");
    assert!(
        e.reason
            .contains("cannot extend the default github-api rule"),
        "{e}"
    );
}

#[test]
fn default_rule_without_paths_rejects_allow() {
    let e = err("proxy:\n  rules:\n    claude:\n      allow: ['POST /v1/messages']\n");
    assert_eq!(e.path, "proxy.rules.claude.allow[0]");
    assert!(e.reason.contains("already applies to every path"), "{e}");
}

#[test]
fn default_rule_bad_allow_reports_user_index() {
    let e = err("proxy:\n  rules:\n    github-api:\n      allow: ['nope']\n");
    assert_eq!(e.path, "proxy.rules.github-api.allow[0]");
}

#[test]
fn user_rule_parses() {
    let cfg = user(GOOD);
    let rule = cfg.proxy.rules.get("gitlab").expect("gitlab");
    assert_eq!(rule.hosts, ["gitlab.com"]);
    assert_eq!(rule.inject.header.as_deref(), Some("PRIVATE-TOKEN"));
}

#[test]
fn unknown_key_names_its_path() {
    let e = err("proxy: {rules: {x: {hosts: [a.com], bogus: 1}}}");
    assert_eq!(e.path, "proxy.rules.x.bogus");
    let e = err("proxy: {rules: {x: {inject: {nope: 1}}}}");
    assert_eq!(e.path, "proxy.rules.x.inject.nope");
}

#[test]
fn direct_is_an_unknown_proxy_key() {
    let e = err("proxy: {direct: [pub.dev]}");
    assert_eq!(e.path, "proxy.direct", "{e}");
    assert!(e.reason.contains("unknown field"), "{e}");
}

#[test]
fn empty_hosts_rejected() {
    let e = err("proxy: {rules: {x: {credential: github, inject: {basic_user: u}}}}");
    assert_eq!(e.path, "proxy.rules.x.hosts");
    let e = err(
        "proxy: {rules: {x: {hosts: ['*.a.com'], credential: github, inject: {basic_user: u}}}}",
    );
    assert_eq!(e.path, "proxy.rules.x.hosts[0]");
}

#[test]
fn unknown_slots_rejected_everywhere() {
    for (frag, path) in [
        ("inject: {form: 'Bearer {secret}{salt}'}", "inject.form"),
        ("inject: {form: 'x'}", "inject.form"),
        (
            "inject: {form: '{secret}'}, allow: ['GET /{owner}']",
            "allow[0]",
        ),
        (
            "inject: {form: '{secret}'}, box_env: {A: '{secret}'}",
            "box_env.A",
        ),
        (
            "inject: {form: '{secret}'}, box_env: {A: '{repo'}",
            "box_env.A",
        ),
    ] {
        let e = err(&format!(
            "proxy: {{rules: {{x: {{hosts: [a.com], credential: github, {frag}}}}}}}"
        ));
        assert_eq!(e.path, format!("proxy.rules.x.{path}"), "{frag}: {e}");
    }
}

#[test]
fn inject_shapes() {
    for frag in [
        "inject: {}",
        "inject: {form: '{secret}', basic_user: u}",
        "inject: {header: X, basic_user: u}",
        "inject: {basic_user: 'a:b'}",
        "inject: {header: 'bad name', form: '{secret}'}",
    ] {
        let e = err(&format!(
            "proxy: {{rules: {{x: {{hosts: [a.com], credential: github, {frag}}}}}}}"
        ));
        assert!(e.path.starts_with("proxy.rules.x.inject"), "{frag}: {e}");
    }
}

#[test]
fn allow_syntax() {
    let base =
        "proxy: {rules: {x: {hosts: [a.com], credential: github, inject: {basic_user: u}, allow: ";
    for bad in ["['get /a']", "['GET a']", "['GET']", "['*', 'GET /a']"] {
        assert!(
            parse_user("u", &format!("{base}{bad}}}}}}}")).is_err(),
            "{bad}"
        );
    }
    for good in ["['*']", "['* /a']", "['GET|HEAD /a/*/b/**']"] {
        assert!(
            parse_user("u", &format!("{base}{good}}}}}}}")).is_ok(),
            "{good}"
        );
    }
}

#[test]
fn a_credential_must_be_a_secret_name() {
    let e = err("proxy: {rules: {x: {hosts: [a.com], credential: NOPE, inject: {basic_user: u}}}}");
    assert_eq!(e.path, "proxy.rules.x.credential");
    assert!(e.reason.contains("secret name"), "{e}");
    user("proxy: {rules: {x: {hosts: [a.com], credential: npm-token, inject: {basic_user: u}}}}");
}

#[test]
fn defaults_are_valid_and_complete() {
    let d = default_rules();
    assert_eq!(
        d.keys().map(String::as_str).collect::<Vec<_>>(),
        ["claude", "github-api", "github-git", "github-uploads"]
    );
    assert!(d.get("claude").expect("claude").allow.is_empty());
    assert_eq!(
        d.get("github-git")
            .and_then(|r| r.inject.basic_user.as_deref()),
        Some("x-access-token")
    );
}

#[test]
fn resolve_fills_repo() {
    let rules = resolve_rules(&ProxyConfig::default(), Some(repo())).expect("resolves");
    let get = |n: &str| rules.iter().find(|r| r.name == n).expect(n);
    let git = get("github-git");
    assert_eq!(
        git.inject,
        ResolvedInject::BasicAuth {
            user: "x-access-token".into()
        }
    );
    assert_eq!(
        git.endpoint_rules.first(),
        Some(&EndpointRule {
            method: "GET".into(),
            path: "/acme/widgets.git/info/refs".into()
        })
    );
    let api = get("github-api");
    assert_eq!(
        api.box_env.get("GH_REPO").map(String::as_str),
        Some("acme/widgets")
    );
    assert!(api.endpoint_rules.contains(&EndpointRule {
        method: "PATCH".into(),
        path: "/repos/acme/widgets/pulls/*".into()
    }));
    assert!(api.endpoint_rules.contains(&EndpointRule {
        method: "POST".into(),
        path: "/graphql".into()
    }));
    let claude = get("claude");
    assert!(claude.endpoint_rules.is_empty());
    assert_eq!(
        claude.inject,
        ResolvedInject::Header {
            header: "Authorization".into(),
            form: "Bearer {secret}".into()
        }
    );
}

#[test]
fn user_rule_overrides_default_inject_and_adds() {
    let cfg = user(&format!(
        "{GOOD}\n    claude: {{inject: {{header: X-Key, form: '{{secret}}'}}}}"
    ));
    let rules = resolve_rules(&cfg.proxy, Some(repo())).expect("resolves");
    let claude = rules.iter().find(|r| r.name == "claude").expect("claude");
    assert_eq!(claude.hosts, ["api.anthropic.com"]);
    assert_eq!(
        claude.inject,
        ResolvedInject::Header {
            header: "X-Key".into(),
            form: "{secret}".into()
        }
    );
    assert_eq!(claude.box_env.len(), 2);
    let gl = rules.iter().find(|r| r.name == "gitlab").expect("gitlab");
    assert_eq!(gl.endpoint_rules.len(), 2);
    assert_eq!(
        gl.endpoint_rules.first().map(|r| r.path.as_str()),
        Some("/api/v4/projects/acme%2Fwidgets/merge_requests/**")
    );
    assert_eq!(
        gl.box_env.get("GL_REPO").map(String::as_str),
        Some("acme/widgets")
    );
    assert_eq!(rules.len(), 5);
}

fn github_api_paths(allow: Option<&str>) -> Vec<String> {
    let doc = allow.map_or_else(String::new, |a| {
        format!("proxy:\n  rules:\n    github-api:\n      allow: ['{a}']\n")
    });
    let cfg = user(&doc);
    let rules = resolve_rules(&cfg.proxy, Some(repo())).expect("resolves");
    let gh = rules.iter().find(|r| r.name == "github-api").expect("gh");
    assert_eq!(gh.hosts, ["api.github.com"]);
    assert_eq!(gh.credential, "github");
    gh.endpoint_rules
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

#[test]
fn user_allow_extends_default_rule() {
    let base = github_api_paths(None);
    let got = github_api_paths(Some("PUT /repos/{repo}/pulls/*/merge"));
    assert_eq!(got.len(), base.len() + 1);
    assert!(got.starts_with(&base));
    assert_eq!(
        got.last().map(String::as_str),
        Some("PUT /repos/acme/widgets/pulls/*/merge")
    );
}

#[test]
fn user_box_env_overrides_default_var() {
    let doc = "proxy:\n  rules:\n    github-api:\n      box_env: {GH_REPO: fixed, MORE: x}\n";
    let cfg = user(doc);
    let rules = resolve_rules(&cfg.proxy, Some(repo())).expect("resolves");
    let gh = rules.iter().find(|r| r.name == "github-api").expect("gh");
    assert_eq!(gh.box_env.get("GH_REPO").map(String::as_str), Some("fixed"));
    assert_eq!(gh.box_env.get("MORE").map(String::as_str), Some("x"));
    assert_eq!(
        gh.box_env.get("GH_TOKEN").map(String::as_str),
        Some("gho_moor-proxy")
    );
}

#[test]
fn github_uploads_rule_injects_github_and_allows_only_the_asset_post() {
    let rules = resolve_rules(&ProxyConfig::default(), Some(("o", "w"))).expect("resolves");
    let up = rules
        .iter()
        .find(|r| r.name == "github-uploads")
        .expect("uploads");
    assert_eq!(up.hosts, ["uploads.github.com"]);
    assert_eq!(up.credential, "github");
    assert_eq!(
        up.inject,
        ResolvedInject::Header {
            header: "Authorization".to_owned(),
            form: "token {secret}".to_owned(),
        }
    );
    let allowed: Vec<String> = up
        .endpoint_rules
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(allowed, ["POST /user-attachments/assets"]);
}

#[test]
fn duplicate_user_allow_collapses() {
    let base = github_api_paths(None);
    assert_eq!(github_api_paths(Some("GET /repos/{repo}")), base);
}

#[test]
fn new_name_rule_stays_whole() {
    let cfg = user(GOOD);
    let rules = resolve_rules(&cfg.proxy, Some(repo())).expect("resolves");
    let gl = rules.iter().find(|r| r.name == "gitlab").expect("gitlab");
    assert_eq!(gl.hosts, ["gitlab.com"]);
    assert_eq!(gl.credential, "gitlab");
    assert_eq!(gl.endpoint_rules.len(), 2);
}

#[test]
fn no_repo_drops_repo_rules() {
    let rules = resolve_rules(&ProxyConfig::default(), None).expect("resolves");
    let names: Vec<_> = rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["claude"]);
}

#[test]
fn unsafe_repo_parts_rejected() {
    for (owner, name) in [
        ("*", "w"),
        ("**", "w"),
        ("..", "w"),
        (".", "w"),
        ("a", "b/c"),
        ("a/b", "w"),
        ("", "w"),
        ("a", ""),
        ("ac\u{fc}", "w"),
        ("a", "w{repo}"),
    ] {
        let e = resolve_rules(&ProxyConfig::default(), Some((owner, name))).expect_err(owner);
        assert!(
            e.reason.contains(&format!("{owner:?}")) || e.reason.contains(&format!("{name:?}")),
            "{owner}/{name}: {e}"
        );
    }
}

fn path_matches(glob: &str, path: &str) -> bool {
    fn go(g: &[&str], p: &[&str]) -> bool {
        match (g.split_first(), p.split_first()) {
            (None, None) => true,
            (Some((&"**", gr)), _) => go(gr, p) || p.split_first().is_some_and(|(_, pr)| go(g, pr)),
            (Some((&"*", gr)), Some((_, pr))) => go(gr, pr),
            (Some((a, gr)), Some((b, pr))) => a == b && go(gr, pr),
            _ => false,
        }
    }
    let g: Vec<_> = glob.split('/').collect();
    let p: Vec<_> = path.split('/').collect();
    go(&g, &p)
}

fn github_api_allows(method: &str, path: &str) -> bool {
    let rules = resolve_rules(&ProxyConfig::default(), Some(repo())).expect("resolves");
    let api = rules.iter().find(|r| r.name == "github-api").expect("api");
    api.endpoint_rules
        .iter()
        .any(|r| (r.method == "*" || r.method == method) && path_matches(&r.path, path))
}

#[test]
fn github_api_leaves_merge_and_ref_writes_unmatched() {
    let r = "/repos/acme/widgets";
    for (m, p) in [
        ("PUT", format!("{r}/pulls/7/merge")),
        ("POST", format!("{r}/pulls/7/merge")),
        ("DELETE", format!("{r}/pulls/7")),
        ("POST", format!("{r}/merges")),
        ("POST", format!("{r}/git/refs")),
        ("PATCH", format!("{r}/git/refs/heads/main")),
        ("DELETE", format!("{r}/git/refs/heads/topic")),
        ("PUT", format!("{r}/pulls/7/merge/extra")),
    ] {
        assert!(!github_api_allows(m, &p), "{m} {p}");
    }
}

#[test]
fn github_api_matches_pr_and_stack_calls() {
    let r = "/repos/acme/widgets";
    for (m, p) in [
        ("GET", r.to_owned()),
        ("POST", format!("{r}/pulls")),
        ("GET", format!("{r}/pulls")),
        ("PATCH", format!("{r}/pulls/7")),
        ("GET", format!("{r}/pulls/7/files")),
        ("POST", format!("{r}/pulls/7/requested_reviewers")),
        ("DELETE", format!("{r}/pulls/7/requested_reviewers")),
        ("POST", format!("{r}/issues/7/comments")),
        ("GET", format!("{r}/commits/abc/check-runs")),
        ("GET", format!("{r}/stacks")),
        ("POST", format!("{r}/stacks")),
        ("GET", format!("{r}/stacks/3")),
        ("POST", format!("{r}/stacks/3/add")),
        ("POST", format!("{r}/stacks/3/unstack")),
        ("POST", "/graphql".to_owned()),
    ] {
        assert!(github_api_allows(m, &p), "{m} {p}");
    }
}

#[test]
fn github_api_allows_run_reads_and_nothing_else_under_actions() {
    let r = "/repos/acme/widgets";
    for p in [
        format!("{r}/actions/runs"),
        format!("{r}/actions/runs/9/jobs"),
        format!("{r}/actions/jobs/4/logs"),
        format!("{r}/check-runs/4/annotations"),
        "/rate_limit".to_owned(),
    ] {
        assert!(github_api_allows("GET", &p), "GET {p}");
    }
    for (m, p) in [
        ("POST", format!("{r}/actions/runs/9/rerun")),
        ("POST", format!("{r}/actions/runs/9/cancel")),
        ("DELETE", format!("{r}/actions/runs/9")),
        ("PUT", format!("{r}/actions/secrets/X")),
        ("GET", "/repos/other/repo/actions/runs".to_owned()),
    ] {
        assert!(!github_api_allows(m, &p), "{m} {p}");
    }
}

#[test]
fn each_default_rule_injects_its_own_secret() {
    let rules = resolve_rules(&ProxyConfig::default(), Some(repo())).expect("resolves");
    let got: Vec<(&str, &str)> = rules
        .iter()
        .map(|r| (r.name.as_str(), r.credential.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("claude", "claude"),
            ("github-api", "github"),
            ("github-git", "github"),
            ("github-uploads", "github"),
        ]
    );
}
