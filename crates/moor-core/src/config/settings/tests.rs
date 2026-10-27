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
fn agent_install_is_on_unless_the_user_turns_it_off() {
    let flags = Flags::default();
    assert!(resolve("{}", Some("{}"), &flags).install_agent);
    assert!(resolve("agent: {install: true}", None, &flags).install_agent);
    assert!(!resolve("agent: {install: false}", Some("{}"), &flags).install_agent);
}

#[test]
fn provider_never_from_repo() {
    let s = resolve("{}", Some("{}"), &Flags::default());
    assert_eq!(s.provider, None);
    let s = resolve("box: {provider: p}", None, &Flags::default());
    assert_eq!(s.provider, Some(Sourced::new("p".to_string(), Layer::User)));
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
    assert_eq!(s.provider, Some(Sourced::new("p".to_string(), Layer::User)));
}

#[test]
fn credentials_are_the_box_credentials_and_mcp_refs() {
    let s = resolve(
        "mcp_servers: {m: {url: 'https://h.example', headers: {A: '${tok}'}}}\nbox_credentials: [used]",
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
        (Layer::Forge, "forge"),
    ];
    for (l, n) in all {
        assert_eq!(l.as_str(), n);
    }
}

#[test]
fn every_user_server_reaches_a_box_that_asks_for_no_machine() {
    let user = "
mcp_servers:
  sim: {url: 'https://sim.example', headers: {Authorization: 'Bearer ${sim_key}'}}
  docs: {url: 'https://docs.example'}
";
    let s = resolve(user, None, &Flags::default());
    assert_eq!(s.mcp.keys().collect::<Vec<_>>(), ["docs", "sim"]);
    assert!(s.credentials.iter().any(|c| c.name == "sim_key"));
}
