//! A start hands the requester's own MCP servers to the agent with a placeholder for each key,
//! and the Proxy a rule that injects that user's key.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;

use moor_core::config::UserMcp;
use moor_core::{BoxId, StartReq};

use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::cx::{put_secret, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};
use crate::vault::Vault;

const KEY: &str = "lin_api_real_key";

fn with_linear(key: bool) -> (World, FakeProxyHost) {
    let fake = FakeProxyHost::new();
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_proxy_host(Arc::new(fake.clone()));
    let store = w.cx.store().unwrap();
    let server = UserMcp::parse("linear", "https://mcp.linear.app/mcp/readonly", None).unwrap();
    store.set_user_mcp(REQUESTER_USER, &server).unwrap();
    if key {
        let vault = Vault::load_or_create(w.cx.vault_key().unwrap()).unwrap();
        put_secret(store, &vault, REQUESTER_USER, "mcp_linear", KEY);
    }
    (w, fake)
}

#[test]
fn the_agent_gets_a_placeholder_and_the_proxy_injects_the_users_key() {
    let (w, fake) = with_linear(true);
    let goal = StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    let v = run(&w.cx, &goal).unwrap();
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    let launch = super::super::launch::launch_req(&w.cx, &row.unwrap().unwrap(), None).unwrap();
    let linear = &launch.mcp["linear"];
    assert_eq!(linear.headers["Authorization"], "Bearer moor-proxy");
    assert!(
        !format!("{launch:?}").contains(KEY),
        "the key never reaches the Box"
    );
    let spec = fake
        .calls()
        .into_iter()
        .find_map(|c| match c {
            ProxyHostCall::Ensure(spec) => Some(spec),
            _ => None,
        })
        .unwrap();
    let rule = spec.rules.iter().find(|r| r.name == "mcp-linear").unwrap();
    assert_eq!(
        (rule.credential.as_str(), spec.user),
        ("mcp_linear", REQUESTER_USER)
    );
    assert_eq!(rule.hosts, ["mcp.linear.app"]);
}

#[test]
fn a_server_without_a_key_is_left_out_and_the_start_says_so() {
    let (w, _) = with_linear(false);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(
        v.warnings_inline.iter().any(|n| n
            == "MCP server linear left out: no key stored for it (add one with /moor mcp add)"),
        "{:?}",
        v.warnings_inline
    );
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    let launch = super::super::launch::launch_req(&w.cx, &row.unwrap().unwrap(), None).unwrap();
    assert!(launch.mcp.is_empty());
}
