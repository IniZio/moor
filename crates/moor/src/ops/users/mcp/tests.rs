#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::request::{SecretValue, UserCmd};
use moor_core::user::SecretName;
use moor_core::ErrorKind;
use serde_json::json;

use super::for_box;
use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};

const KEY: &str = "lin_api_real_key";

fn add(name: &str, url: &str, key: Option<&str>) -> UserCmd {
    UserCmd::McpAdd {
        name: name.to_owned(),
        url: url.to_owned(),
        header: None,
        key: key.map(|k| SecretValue::new(k.into())),
    }
}

fn enrolled(f: &Fixture, who: &str) -> moor_core::user::UserId {
    f.cx.store().unwrap().enroll(&id(who)).unwrap().0
}

#[test]
fn a_users_servers_and_keys_are_theirs_alone() {
    let f = fixture();
    let a = enrolled(&f, "slack:T1:UA");
    let b = enrolled(&f, "slack:T1:UB");
    let linear = add("linear", "https://mcp.linear.app/mcp/readonly", Some(KEY));
    run(&f.cx, &f.vault, &id("slack:T1:UA"), &linear).unwrap();

    let listed = run(&f.cx, &f.vault, &id("slack:T1:UA"), &UserCmd::McpList).unwrap();
    assert_eq!(
        listed,
        json!({"servers": [{"name": "linear", "url": "https://mcp.linear.app/mcp/readonly",
            "header": "Authorization", "secret": "mcp_linear", "has_key": true}]})
    );
    assert!(
        !listed.to_string().contains(KEY),
        "a list never shows a key"
    );
    let theirs = run(&f.cx, &f.vault, &id("slack:T1:UB"), &UserCmd::McpList).unwrap();
    assert_eq!(theirs, json!({"servers": []}), "B never sees A's server");

    let store = f.cx.store().unwrap();
    let secret = SecretName::parse("mcp_linear").unwrap();
    assert_eq!(
        store
            .user_secret(&f.vault, a, &secret)
            .unwrap()
            .unwrap()
            .expose(),
        KEY.as_bytes()
    );
    assert!(store.user_secret(&f.vault, b, &secret).unwrap().is_none());
    assert!(
        for_box(&f.cx, Some(b)).unwrap().servers.is_empty(),
        "B's Box never gets A's server"
    );
    assert!(for_box(&f.cx, Some(a))
        .unwrap()
        .servers
        .contains_key("linear"));

    let removed = run(
        &f.cx,
        &f.vault,
        &id("slack:T1:UB"),
        &UserCmd::McpRemove {
            name: "linear".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(removed["changed"], false);
    assert!(
        store.user_secret(&f.vault, a, &secret).unwrap().is_some(),
        "B's remove leaves A's key"
    );
}

#[test]
fn the_box_sees_a_placeholder_never_the_key() {
    let f = fixture();
    let a = enrolled(&f, "local:2000");
    let cmd = add("linear", "https://mcp.linear.app/mcp/readonly", Some(KEY));
    run(&f.cx, &f.vault, &id("local:2000"), &cmd).unwrap();
    let server = for_box(&f.cx, Some(a)).unwrap().servers["linear"].in_box();
    assert_eq!(server.headers["Authorization"], "Bearer moor-proxy");
    assert!(!format!("{server:?}").contains(KEY));
    let rule = for_box(&f.cx, Some(a)).unwrap().servers["linear"]
        .proxy_rule()
        .unwrap();
    assert_eq!(rule.credential, "mcp_linear", "the Proxy injects A's key");
}

#[test]
fn a_server_without_a_key_is_left_out_and_named() {
    let f = fixture();
    let a = enrolled(&f, "local:2000");
    let cmd = add("docs", "https://docs.example/mcp", None);
    run(&f.cx, &f.vault, &id("local:2000"), &cmd).unwrap();
    let got = for_box(&f.cx, Some(a)).unwrap();
    assert!(got.servers.is_empty());
    assert_eq!(
        got.notes,
        ["MCP server docs left out: you have no secret mcp_docs"]
    );
}

#[test]
fn remove_drops_the_server_and_its_key() {
    let f = fixture();
    let a = enrolled(&f, "local:2000");
    let cmd = add("linear", "https://mcp.linear.app/mcp/readonly", Some(KEY));
    run(&f.cx, &f.vault, &id("local:2000"), &cmd).unwrap();
    let rm = UserCmd::McpRemove {
        name: "linear".to_owned(),
    };
    let out = run(&f.cx, &f.vault, &id("local:2000"), &rm).unwrap();
    assert_eq!(out["changed"], true);
    let store = f.cx.store().unwrap();
    assert!(store.user_mcp(a).unwrap().is_empty());
    assert!(store.user_secret_names(a).unwrap().is_empty());
}

#[test]
fn a_bad_url_or_name_is_refused_before_anything_is_stored() {
    let f = fixture();
    let a = enrolled(&f, "local:2000");
    for cmd in [
        add("linear", "http://mcp.linear.app/mcp", Some(KEY)),
        add("Linear", "https://mcp.linear.app/mcp", Some(KEY)),
    ] {
        let err = run(&f.cx, &f.vault, &id("local:2000"), &cmd).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{err:?}");
    }
    assert!(f
        .cx
        .store()
        .unwrap()
        .user_secret_names(a)
        .unwrap()
        .is_empty());
}
