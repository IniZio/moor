use serde_json::json;

use super::may;

#[test]
fn no_config_asks_for_nothing_extra() {
    assert_eq!(may(None), json!({ "allow": {} }));
    assert_eq!(may(Some("  \n")), json!({ "allow": {} }));
}

#[test]
fn the_entries_come_back_as_written_under_their_rule() {
    let text = "proxy:\n  allow:\n    github-api:\n      - POST /repos/{repo}/issues/*/comments\n      - PATCH /repos/{repo}/issues/comments/*\n";
    assert_eq!(
        may(Some(text)),
        json!({ "allow": { "github-api": [
            "POST /repos/{repo}/issues/*/comments",
            "PATCH /repos/{repo}/issues/comments/*"
        ] } })
    );
}

#[test]
fn a_config_with_no_proxy_section_asks_for_nothing_extra() {
    assert_eq!(may(Some("setup: [make]\n")), json!({ "allow": {} }));
}

#[test]
fn a_config_that_no_longer_parses_says_why_instead_of_showing_nothing() {
    let shown = may(Some("proxy: {allow: {nope: [\"GET /x\"]}}"));
    assert!(shown.get("allow").is_none(), "{shown}");
    let why = shown
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or_default();
    assert!(why.contains("proxy.allow.nope"), "{shown}");
}
