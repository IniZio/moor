//! Tests for proxy rule parsing, validation and resolution.

use super::*;
use crate::config::CredentialSource;
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
credentials: {tok: {env: X}}
proxy:
  rules:
    gitlab:
      hosts: [gitlab.com]
      credential: tok
      inject: {header: PRIVATE-TOKEN, form: '{secret}'}
      allow: ['GET|POST /api/v4/projects/{repo:urlenc}/merge_requests/**']
      box_env: {GL_REPO: '{repo}'}
";

fn repo() -> (&'static str, &'static str) {
    ("acme", "widgets")
}

fn creds() -> BTreeMap<String, CredentialSource> {
    let src = CredentialSource {
        env: Some("X".into()),
        ..CredentialSource::default()
    };
    BTreeMap::from([("CLAUDE_CODE_OAUTH_TOKEN".to_owned(), src)])
}

#[test]
fn default_rule_without_its_credential_errors_on_resolve() {
    let e = resolve_rules(&ProxyConfig::default(), &BTreeMap::new(), None).expect_err("missing");
    assert_eq!(e.path, "credentials");
    assert_eq!(
        e.reason,
        "the default claude rule needs credentials.CLAUDE_CODE_OAUTH_TOKEN; add it under credentials:"
    );
}

#[test]
fn user_rule_without_its_credential_keeps_the_rule_path() {
    let e = err(
        "proxy:\n  rules:\n    mine:\n      hosts: [api.example.com]\n      credential: MINE\n      inject: {form: 'Bearer {secret}'}\n",
    );
    assert_eq!(e.path, "proxy.rules.mine.credential");
    assert!(e.reason.starts_with("unknown credential \"MINE\""), "{e}");
}

#[test]
fn overridden_default_rule_keeps_the_rule_path() {
    let e = err(
        "proxy:\n  rules:\n    claude:\n      hosts: [api.example.com]\n      credential: OTHER\n      inject: {form: 'Bearer {secret}'}\n",
    );
    assert_eq!(e.path, "proxy.rules.claude.credential");
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
fn empty_hosts_rejected() {
    let e = err("proxy: {rules: {x: {credential: forge, inject: {basic_user: u}}}}");
    assert_eq!(e.path, "proxy.rules.x.hosts");
    let e = err(
        "proxy: {rules: {x: {hosts: ['*.a.com'], credential: forge, inject: {basic_user: u}}}}",
    );
    assert_eq!(e.path, "proxy.rules.x.hosts[0]");
}

#[test]
fn unknown_slots_rejected_everywhere() {
    for (frag, path) in [
        ("inject: {form: 'Bearer {secret}{nonce}'}", "inject.form"),
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
            "proxy: {{rules: {{x: {{hosts: [a.com], credential: forge, {frag}}}}}}}"
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
            "proxy: {{rules: {{x: {{hosts: [a.com], credential: forge, {frag}}}}}}}"
        ));
        assert!(e.path.starts_with("proxy.rules.x.inject"), "{frag}: {e}");
    }
}

#[test]
fn allow_syntax() {
    let base =
        "proxy: {rules: {x: {hosts: [a.com], credential: forge, inject: {basic_user: u}, allow: ";
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
fn unknown_credential_rejected_forge_allowed() {
    let e = err("proxy: {rules: {x: {hosts: [a.com], credential: nope, inject: {basic_user: u}}}}");
    assert_eq!(e.path, "proxy.rules.x.credential");
    user("proxy: {rules: {x: {hosts: [a.com], credential: forge, inject: {basic_user: u}}}}");
}

#[test]
fn host_in_egress_allow_names_both_paths() {
    for allow in ["a.com", "A.com", "*.a.com"] {
        let host = if allow == "*.a.com" {
            "x.a.com"
        } else {
            "a.com"
        };
        let e = err(&format!(
            "egress: {{allow: [other.org, '{allow}']}}\nproxy: {{rules: {{x: {{hosts: [{host}], credential: forge, inject: {{basic_user: u}}}}}}}}"
        ));
        assert_eq!(e.path, "proxy.rules.x.hosts[0]", "{allow}");
        assert!(e.reason.contains("egress.allow[1]"), "{e}");
    }
    user("egress: {allow: [b.com]}\nproxy: {rules: {x: {hosts: [a.com], credential: forge, inject: {basic_user: u}}}}");
}

#[test]
fn defaults_are_valid_and_complete() {
    let d = default_rules();
    assert_eq!(
        d.keys().map(String::as_str).collect::<Vec<_>>(),
        ["claude", "github-api", "github-git"]
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
    let rules = resolve_rules(&ProxyConfig::default(), &creds(), Some(repo())).expect("resolves");
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
fn user_rule_replaces_default_and_adds() {
    let cfg = user(&format!(
        "{GOOD}\n    claude: {{hosts: [llm.example.com], credential: tok, inject: {{form: '{{secret}}'}}}}"
    ));
    let rules = resolve_rules(&cfg.proxy, &cfg.credentials, Some(repo())).expect("resolves");
    let claude = rules.iter().find(|r| r.name == "claude").expect("claude");
    assert_eq!(claude.hosts, ["llm.example.com"]);
    assert!(claude.box_env.is_empty());
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
    assert_eq!(rules.len(), 4);
}

#[test]
fn no_repo_drops_repo_rules() {
    let rules = resolve_rules(&ProxyConfig::default(), &creds(), None).expect("resolves");
    let names: Vec<_> = rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["claude"]);
}

#[test]
fn default_hosts_conflict_with_egress_allow() {
    for allow in ["api.github.com", "github.com", "'*.anthropic.com'"] {
        let e = err(&format!("egress: {{allow: [{allow}]}}"));
        assert!(e.path.starts_with("proxy.rules."), "{allow}: {e}");
        assert!(e.reason.contains("egress.allow[0]"), "{e}");
    }
    user("proxy: {rules: {github-api: {hosts: [gh.example.com], credential: forge, inject: {basic_user: u}}}}\negress: {allow: [api.github.com]}");
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
        let e =
            resolve_rules(&ProxyConfig::default(), &creds(), Some((owner, name))).expect_err(owner);
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
    let rules = resolve_rules(&ProxyConfig::default(), &creds(), Some(repo())).expect("resolves");
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
