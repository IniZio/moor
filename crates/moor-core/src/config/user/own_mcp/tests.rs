#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::collections::{BTreeMap, BTreeSet};

use super::{box_mcp, UserMcp};
use crate::config::McpServer;
use crate::user::SecretName;

fn shared(name: &str, secret: &str) -> (String, McpServer) {
    (
        name.to_owned(),
        McpServer {
            url: Some(format!("https://{name}.shared.example/mcp")),
            headers: BTreeMap::from([(
                "Authorization".to_owned(),
                format!("Bearer ${{{secret}}}"),
            )]),
            ..McpServer::default()
        },
    )
}

fn have(names: &[&str]) -> BTreeSet<SecretName> {
    names
        .iter()
        .map(|n| SecretName::parse(n).unwrap())
        .collect()
}

fn linear() -> UserMcp {
    UserMcp::parse("linear", "https://mcp.linear.app/mcp/readonly", None).unwrap()
}

#[test]
fn a_user_server_names_its_own_vault_secret_in_a_bearer_header() {
    let s = linear().server();
    assert_eq!(
        s.url.as_deref(),
        Some("https://mcp.linear.app/mcp/readonly")
    );
    assert_eq!(s.headers["Authorization"], "Bearer ${mcp_linear}");
    let rule = s.proxy_rule().unwrap();
    assert_eq!(rule.hosts, ["mcp.linear.app"]);
    assert_eq!(rule.credential, "mcp_linear");
    assert_eq!(s.in_box().headers["Authorization"], "Bearer moor-proxy");
}

#[test]
fn another_header_carries_the_key_alone() {
    let s = UserMcp::parse("docs", "https://docs.example/mcp", Some("X-Api-Key"))
        .unwrap()
        .server();
    assert_eq!(s.headers["X-Api-Key"], "${mcp_docs}");
}

#[test]
fn names_urls_and_headers_are_checked() {
    for (name, url, header) in [
        ("Linear", "https://a.example/mcp", None),
        ("a b", "https://a.example/mcp", None),
        ("", "https://a.example/mcp", None),
        ("ok", "http://a.example/mcp", None),
        ("ok", "https://user@a.example/mcp", None),
        ("ok", "ftp://a.example", None),
        ("ok", "https://a.example/mcp", Some("Bad Header")),
    ] {
        assert!(
            UserMcp::parse(name, url, header).is_err(),
            "{name} {url} {header:?}"
        );
    }
    assert!(UserMcp::parse("local", "http://localhost:8080/mcp", None).is_ok());
}

#[test]
fn the_users_server_wins_a_name_clash_and_says_so() {
    let shared = BTreeMap::from([shared("linear", "linear_ro"), shared("docs", "docs_key")]);
    let got = box_mcp(&shared, &[linear()], &have(&["mcp_linear", "docs_key"]));
    assert_eq!(
        got.servers.keys().collect::<Vec<_>>(),
        ["docs", "linear"],
        "both kept"
    );
    assert_eq!(
        got.servers["linear"].url.as_deref(),
        Some("https://mcp.linear.app/mcp/readonly"),
        "the user's own replaced the shared one"
    );
    assert_eq!(
        got.notes,
        ["your MCP server linear replaces the shared one of that name"]
    );
}

#[test]
fn a_server_whose_key_is_missing_is_left_out_with_one_note() {
    let shared = BTreeMap::from([shared("docs", "docs_key"), shared("open", "open_key")]);
    let got = box_mcp(&shared, &[linear()], &have(&["open_key"]));
    assert_eq!(got.servers.keys().collect::<Vec<_>>(), ["open"]);
    assert_eq!(
        got.notes,
        ["MCP server docs, linear left out: no key stored for it (add one with /moor mcp add)"]
    );
}

#[test]
fn nothing_to_say_when_every_server_resolves() {
    let got = box_mcp(&BTreeMap::new(), &[linear()], &have(&["mcp_linear"]));
    assert_eq!(got.servers.len(), 1);
    assert!(got.notes.is_empty());
}
