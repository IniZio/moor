//! The per-Box config override on the wire.

use super::*;
use serde_json::json;

/// Valid Box name.
fn n(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).expect("valid name")
}

/// A controller one release older refuses a field it does not know, so a request with no
/// override must not carry the key, and one with an override must round-trip it.
#[test]
fn config_is_left_out_when_absent_and_kept_when_given() {
    let start = |config: Option<&str>| StartReq {
        placement: crate::decide::schedule::Placement::FirstOnly,
        cold: false,
        config: config.map(str::to_owned),
        name: n("x"),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        resume: false,
        repo: None,
        parent: None,
        also: None,
        identity: None,
    };
    let bare = serde_json::to_value(Request::Start(start(None))).expect("ser");
    assert!(bare.get("config").is_none(), "{bare}");
    let given = serde_json::to_value(Request::Start(start(Some("env: {A: b}\n")))).expect("ser");
    assert_eq!(given.get("config"), Some(&json!("env: {A: b}\n")));
    let back: Request = serde_json::from_value(given).expect("de");
    assert_eq!(back, Request::Start(start(Some("env: {A: b}\n"))));

    let set = Request::BoxConfigSet(BoxConfigSetReq {
        box_name: n("x"),
        config: None,
    });
    assert_eq!(set.cmd(), "box-config-set");
    assert!(serde_json::to_value(&set)
        .expect("ser")
        .get("config")
        .is_none());
    assert_eq!(
        set.names().len(),
        1,
        "it names the Box, so authz checks its owner"
    );
}
