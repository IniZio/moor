#![allow(clippy::indexing_slicing, reason = "test assertions")]

use crate::config::proxy::{resolve_rules, ResolvedInject};

use std::collections::BTreeSet;

use crate::user::SecretName;

use super::{err, ok};

fn held(names: &[&str]) -> BTreeSet<SecretName> {
    names.iter().map(|n| SecretName::parse(n).unwrap()).collect()
}

const LINEAR: &str = "mcp_servers: {tracker: {url: 'https://mcp.example.test/mcp', headers: {Authorization: 'Bearer ${linear}'}}}";

#[test]
fn the_box_sees_a_placeholder_where_a_header_names_a_secret() {
    let cfg = ok(LINEAR);
    let boxed = cfg.mcp_servers["tracker"].in_box();
    assert_eq!(boxed.headers["Authorization"], "Bearer moor-proxy");
    assert_eq!(boxed.url.as_deref(), Some("https://mcp.example.test/mcp"));
}

#[test]
fn the_proxy_injects_the_named_secret_for_the_server_host() {
    let cfg = ok(LINEAR);
    let rules = resolve_rules(&cfg.box_proxy(&held(&["linear"])), None).unwrap();
    let rule = rules.iter().find(|r| r.name == "mcp-tracker").unwrap();
    assert_eq!(rule.hosts, ["mcp.example.test"]);
    assert_eq!(rule.credential, "linear");
    assert_eq!(
        rule.inject,
        ResolvedInject::Header {
            header: "Authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        }
    );
    assert!(ok("mcp_servers: {s: {command: x}}")
        .box_proxy(&held(&[]))
        .rules
        .is_empty());
}

#[test]
fn a_secret_that_would_land_in_the_box_is_refused() {
    for (doc, at) in [
        ("{command: x, args: ['${t}']}", "mcp_servers.m.args[0]"),
        ("{command: x, env: {K: '${t}'}}", "mcp_servers.m.env.K"),
        ("{url: 'https://h.example/${t}'}", "mcp_servers.m.url"),
        (
            "{url: 'http://h.example', headers: {A: '${t}'}}",
            "mcp_servers.m.url",
        ),
        (
            "{url: 'https://h.example', headers: {A: '${t}', B: '${u}'}}",
            "mcp_servers.m.headers",
        ),
        (
            "{url: 'https://h.example', headers: {A: '${t}${u}'}}",
            "mcp_servers.m.headers.A",
        ),
        (
            "{url: 'https://h.example', allow_tools: ['get issue']}",
            "mcp_servers.m.allow_tools[0]",
        ),
    ] {
        let e = err(&format!("mcp_servers: {{m: {doc}}}"));
        assert_eq!(e.path, at, "{doc}: {e}");
    }
    let clash = err("mcp_servers: {m: {command: x}}\nproxy: {rules: {mcp-m: {hosts: [h.example], credential: t, inject: {form: '{secret}'}}}}");
    assert_eq!(clash.path, "mcp_servers.m", "{clash}");
}

#[test]
fn a_server_whose_secret_is_not_held_gets_no_rule_and_no_entry() {
    let cfg = ok(LINEAR);
    let none = held(&[]);
    assert!(cfg.box_proxy(&none).rules.is_empty());
    assert!(cfg.box_mcp(&none).is_empty());
    assert_eq!(
        cfg.mcp_left_out(&none),
        [("tracker".to_owned(), vec!["linear".to_owned()])]
    );
}

#[test]
fn a_server_whose_secret_is_held_is_kept_and_not_left_out() {
    let cfg = ok(LINEAR);
    let have = held(&["linear"]);
    assert!(cfg.box_proxy(&have).rules.contains_key("mcp-tracker"));
    assert_eq!(cfg.box_mcp(&have)["tracker"].headers["Authorization"], "Bearer moor-proxy");
    assert!(cfg.mcp_left_out(&have).is_empty());
}
