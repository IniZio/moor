//! Precedence and credential planning for `BoxSettings::resolve`.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test assertions")]

use super::*;
use crate::config::{parse_repo, parse_user};

fn user(t: &str) -> UserConfig {
    parse_user("u.yaml", t).unwrap_or_else(|e| panic!("{t}: {e}"))
}

fn repo(t: &str) -> RepoConfig {
    parse_repo("r.yaml", t).unwrap_or_else(|e| panic!("{t}: {e}"))
}

fn resolve(u: &str, r: Option<&str>, flags: &Flags) -> BoxSettings {
    let rc = r.map(repo);
    BoxSettings::resolve(&user(u), rc.as_ref(), flags)
}

fn flag_model(m: &str) -> Flags {
    Flags {
        model: Some(m.into()),
    }
}

#[test]
fn model_precedence() {
    type Case = (
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<(&'static str, Layer)>,
    );
    let cases: [Case; 4] = [
        (
            "agent: {model: u}",
            "{}",
            Some("f"),
            Some(("f", Layer::Flag)),
        ),
        ("agent: {model: u}", "{}", None, Some(("u", Layer::User))),
        ("{}", "{}", Some("f"), Some(("f", Layer::Flag))),
        ("{}", "{}", None, None),
    ];
    for (u, r, f, want) in cases {
        let flags = Flags {
            model: f.map(String::from),
        };
        let got = resolve(u, Some(r), &flags).model;
        let want = want.map(|(v, s)| Sourced::new(v.to_string(), s));
        assert_eq!(got, want, "{u} / {r} / {f:?}");
    }
    assert_eq!(
        resolve("agent: {model: u}", None, &flag_model("f")).model,
        Some(Sourced::new("f".to_string(), Layer::Flag))
    );
}

#[test]
fn provider_never_from_repo() {
    let s = resolve("{}", Some("{}"), &Flags::default());
    assert_eq!(
        s.provider,
        Sourced::new("local-container".to_string(), Layer::Default)
    );
    let s = resolve("box: {provider: p}", None, &Flags::default());
    assert_eq!(s.provider, Sourced::new("p".to_string(), Layer::User));
}

#[test]
fn size_decision_58() {
    let cases = [
        ("box: {size: medium}", "{}", BoxSize::Medium, Layer::User),
        (
            "box: {size: medium}",
            "box: {size: large}",
            BoxSize::Large,
            Layer::Repo,
        ),
        ("{}", "box: {size: xlarge}", BoxSize::Xlarge, Layer::Repo),
        ("{}", "{}", BoxSize::Small, Layer::Default),
    ];
    for (u, r, size, layer) in cases {
        assert_eq!(
            resolve(u, Some(r), &Flags::default()).size,
            Sourced::new(size, layer),
            "{u} / {r}"
        );
    }
    let s = resolve(
        "box: {provider: p, size: small}",
        Some("box: {size: large}"),
        &Flags::default(),
    );
    assert_eq!(s.size, Sourced::new(BoxSize::Large, Layer::Repo));
    assert_eq!(s.provider, Sourced::new("p".to_string(), Layer::User));
}

#[test]
fn credentials_are_the_box_credentials_and_mcp_refs() {
    let s = resolve(
        "mcp_servers: {m: {url: 'http://h/${tok}'}}\nbox_credentials: [used]",
        Some("box_credentials: [boxc]"),
        &Flags::default(),
    );
    let got: Vec<_> = s
        .credentials
        .iter()
        .map(|c| (c.name.as_str(), c.layer, c.boxed))
        .collect();
    assert_eq!(
        got,
        [
            ("boxc", Layer::Repo, true),
            ("tok", Layer::User, false),
            ("used", Layer::User, true),
        ]
    );
    assert_eq!(s.box_credentials, ["boxc"]);
}

#[test]
fn repo_fields_and_user_fields_pass_through() {
    let s = resolve("", Some("setup: [a]\nenv: {K: v}"), &Flags::default());
    assert_eq!(s.setup, ["a"]);
    assert_eq!(s.env["K"], "v");
}

#[test]
fn layer_names() {
    let all = [
        (Layer::Flag, "flag"),
        (Layer::Repo, "repo"),
        (Layer::User, "user"),
        (Layer::Default, "default"),
        (Layer::Git, "git"),
    ];
    for (l, n) in all {
        assert_eq!(l.as_str(), n);
    }
}

/// The `NO_PROXY` forms of the effective direct list for user `u` and repo `r`.
fn direct(u: &str, r: Option<&str>) -> Vec<String> {
    resolve(u, r, &Flags::default())
        .direct
        .iter()
        .map(crate::name::DirectHost::no_proxy)
        .collect()
}

const BUILTIN: [&str; 19] = [
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "repo.maven.apache.org",
    "repo1.maven.org",
    ".gradle.org",
    "proxy.golang.org",
    "sum.golang.org",
    "rubygems.org",
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
    "dl.google.com",
    "downloads.claude.ai",
];

/// A user rule that sends registry.npmjs.org through the Proxy with a credential.
const NPM_RULE: &str =
    "rules: {npm: {hosts: [registry.npmjs.org], credential: forge, inject: {form: 'Bearer {secret}'}}}";

#[test]
fn with_no_config_the_direct_list_is_the_builtin_registries() {
    assert_eq!(direct("", None), BUILTIN);
}

#[test]
fn direct_hosts_are_builtin_then_user_then_repo_each_once() {
    let got = direct(
        "proxy: {direct: [pub.dev, pypi.org, .gradle.org]}",
        Some("proxy: {direct: [cdn.example.org, pub.dev, dl.google.com]}"),
    );
    let mut want = BUILTIN.to_vec();
    want.extend(["pub.dev", "cdn.example.org"]);
    assert_eq!(got, want);
}

#[test]
fn a_builtin_host_a_user_rule_serves_is_dropped_and_stays_proxied() {
    let doc = format!("proxy: {{{NPM_RULE}}}");
    let s = resolve(&doc, None, &Flags::default());
    let forms: Vec<String> = s
        .direct
        .iter()
        .map(crate::name::DirectHost::no_proxy)
        .collect();
    let want: Vec<&str> = BUILTIN
        .into_iter()
        .filter(|h| *h != "registry.npmjs.org")
        .collect();
    assert_eq!(forms, want);
    let rules = user(&doc).proxy.merged_rules();
    let hosts: Vec<&str> = rules
        .values()
        .flat_map(|r| r.hosts.iter().map(String::as_str))
        .collect();
    assert!(hosts.contains(&"registry.npmjs.org"), "{hosts:?}");
    assert!(
        super::super::proxy::check_direct(&s.direct, "proxy.direct", &hosts).is_ok(),
        "the proxy step accepts the effective list"
    );
}

#[test]
fn a_builtin_rule_host_under_a_suffix_entry_drops_the_whole_entry() {
    let got = direct(
        "proxy: {rules: {gp: {hosts: [plugins.gradle.org], credential: forge, inject: {form: '{secret}'}}}}",
        None,
    );
    assert!(!got.iter().any(|h| h == ".gradle.org"), "{got:?}");
    assert_eq!(got.len(), BUILTIN.len() - 1, "{got:?}");
}

#[test]
fn an_explicit_user_entry_a_rule_serves_is_still_refused() {
    let doc = format!("proxy: {{direct: [registry.npmjs.org], {NPM_RULE}}}");
    let e = parse_user("u.yaml", &doc).expect_err("a rule host listed direct");
    assert_eq!(e.path, "proxy.direct[0]", "{e}");
    assert!(e.to_string().contains("covers registry.npmjs.org"), "{e}");
}

#[test]
fn an_explicit_repo_entry_a_rule_serves_stays_listed_for_the_proxy_step_to_refuse() {
    let doc = format!("proxy: {{{NPM_RULE}}}");
    let s = resolve(
        &doc,
        Some("proxy: {direct: [pub.dev, registry.npmjs.org]}"),
        &Flags::default(),
    );
    let i = s
        .direct
        .iter()
        .position(|h| h.no_proxy() == "registry.npmjs.org")
        .unwrap_or_else(|| panic!("repo entry kept: {:?}", s.direct));
    let e = super::super::proxy::check_direct(&s.direct, "proxy.direct", &["registry.npmjs.org"])
        .expect_err("the proxy step refuses it");
    assert_eq!(e.path, format!("proxy.direct[{i}]"), "{e}");
}

#[test]
fn default_direct_false_in_user_config_leaves_only_user_and_repo_entries() {
    let got = direct(
        "proxy: {default_direct: false, direct: [pub.dev]}",
        Some("proxy: {direct: [dl.google.com]}"),
    );
    assert_eq!(got, ["pub.dev", "dl.google.com"]);
    assert_eq!(
        direct("proxy: {default_direct: false}", None),
        Vec::<String>::new()
    );
}

#[test]
fn default_direct_in_moor_yaml_is_refused() {
    let e = parse_repo("r.yaml", "proxy: {default_direct: false}")
        .expect_err("a repo cannot turn off the built-in list");
    assert_eq!(e.path, "proxy.default_direct", "{e}");
    assert!(e.to_string().contains("unknown field"), "{e}");
}
