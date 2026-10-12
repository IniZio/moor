//! Precedence and credential planning for `TaskSettings::resolve`.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test assertions")]

use super::*;
use crate::config::{parse_repo, parse_user};

fn user(t: &str) -> UserConfig {
    parse_user("u.yaml", t).unwrap_or_else(|e| panic!("{t}: {e}"))
}

fn repo(t: &str) -> RepoConfig {
    parse_repo("r.yaml", t).unwrap_or_else(|e| panic!("{t}: {e}"))
}

fn resolve(u: &str, r: Option<&str>, flags: &Flags) -> TaskSettings {
    let rc = r.map(repo);
    TaskSettings::resolve(&user(u), rc.as_ref(), flags)
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
    let cases: [Case; 5] = [
        (
            "agent: {model: u}",
            "agent: {model: r}",
            Some("f"),
            Some(("f", Layer::Flag)),
        ),
        (
            "agent: {model: u}",
            "agent: {model: r}",
            None,
            Some(("r", Layer::Repo)),
        ),
        ("agent: {model: u}", "{}", None, Some(("u", Layer::User))),
        ("{}", "agent: {model: r}", None, Some(("r", Layer::Repo))),
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
fn git_identity_from_user_only() {
    let s = resolve("git: {name: U}", Some("{}"), &Flags::default());
    assert_eq!(s.git_name, Some(Sourced::new("U".to_string(), Layer::User)));
    assert_eq!(s.git_email, None);
    let s = resolve("{}", None, &Flags::default());
    assert_eq!((s.git_name, s.git_email), (None, None));
}

#[test]
fn credentials_union_and_used() {
    let s = resolve(
        "credentials: {UNRELATED: {env: A}, USED: {env: B}, TOK: {env: C}}\n\
         mcp_servers: {m: {url: 'http://h/${TOK}'}}\n\
         box_credentials: [USED]",
        Some("box_credentials: [BOXC]"),
        &Flags::default(),
    );
    let got: Vec<_> = s
        .credentials
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.layer,
                c.used,
                c.boxed,
                c.source.is_some(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("BOXC", Layer::Repo, true, true, false),
            ("TOK", Layer::User, true, false, true),
            ("UNRELATED", Layer::User, false, false, true),
            ("USED", Layer::User, true, true, true),
        ]
    );
    let used: Vec<_> = s.used_credentials().map(|c| c.name.as_str()).collect();
    assert_eq!(used, ["BOXC", "TOK", "USED"]);
}

#[test]
fn boxed_from_repo_and_user() {
    let s = resolve(
        "credentials: {NPM: {env: N}, GEM: {env: G}}",
        Some("box_credentials: [NPM]"),
        &Flags::default(),
    );
    let boxed: Vec<_> = s
        .credentials
        .iter()
        .map(|c| (c.name.as_str(), c.boxed))
        .collect();
    assert_eq!(boxed, [("GEM", false), ("NPM", true)]);
    assert_eq!(s.box_credentials, ["NPM"]);
    let s = resolve(
        "credentials: {GEM: {env: G}}\nbox_credentials: [GEM]",
        None,
        &Flags::default(),
    );
    assert!(s.credentials[0].boxed && s.credentials[0].used);
    assert!(s.box_credentials.is_empty());
}

#[test]
fn repo_fields_and_user_fields_pass_through() {
    let s = resolve(
        "box: {enforce_egress: true}\negress: {allow: [u.io]}\ngithub: {identity: bot}",
        Some("egress: [r.io]\nsetup: [a]\nenv: {K: v}"),
        &Flags::default(),
    );
    assert!(s.enforce_egress);
    assert_eq!(
        (s.egress, s.user_egress),
        (vec!["r.io".into()], vec!["u.io".into()])
    );
    assert_eq!(s.identity, Identity::Bot);
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
    ];
    for (l, n) in all {
        assert_eq!(l.as_str(), n);
    }
}
