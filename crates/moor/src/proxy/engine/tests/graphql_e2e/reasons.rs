//! Each `credential_withheld` event says why the credential stayed behind.

use super::any_repo::{default_rule_rig, send};
use super::rejected::events_after;
use super::{query, rig_with, Rig};

/// The payload of the one `credential_withheld` event recorded so far.
fn only_withheld(rig: &Rig) -> String {
    let events = events_after(rig, "credential_withheld");
    let withheld: Vec<&String> = events
        .iter()
        .filter(|(k, _)| k == "credential_withheld")
        .map(|(_, p)| p)
        .collect();
    let [only] = withheld.as_slice() else {
        unreachable!("one report expected: {events:?}")
    };
    (*only).clone()
}

/// The `reason` field of an event payload.
fn reason(payload: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap();
    v.get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| unreachable!("no reason in {payload}"))
        .to_owned()
}

#[test]
fn a_rest_request_outside_the_allow_list_says_so() {
    let rig = default_rule_rig();
    send(
        &rig,
        "GET",
        "/repos/cli/cli/pulls",
        Some("token gho_moor-proxy"),
    );
    let why = reason(&only_withheld(&rig));
    assert_eq!(why, "no allow entry matches the request");
    rig.stop();
}

#[test]
fn a_graphql_query_on_another_repo_names_the_field_but_not_the_repo() {
    let rig = rig_with(Vec::new());
    let (status, _) = query(&rig.client(), "victim", "secrets");
    assert_eq!(status, 401);
    let why = reason(&only_withheld(&rig));
    assert!(why.contains("field `repository`"), "{why}");
    assert!(!why.contains("victim") && !why.contains("secrets"), "{why}");
    rig.stop();
}

#[test]
fn a_failed_node_check_says_so() {
    let rig = rig_with(Vec::new());
    let resp = rig
        .client()
        .post("https://api.github.com/graphql")
        .header("Authorization", "token placeholder-not-real")
        .header("Content-Type", "application/json")
        .send(r#"{"query":"query{node(id:\"PR_x\"){id}}"}"#)
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let why = reason(&only_withheld(&rig));
    assert_eq!(why, "node id lookup failed");
    rig.stop();
}

#[test]
fn another_repo_id_says_it_is_not_a_box_repo() {
    let rig = default_rule_rig();
    send(
        &rig,
        "GET",
        "/repositories/900/pulls",
        Some("token gho_moor-proxy"),
    );
    let why = reason(&only_withheld(&rig));
    assert_eq!(why, "repository id is not a Box repo");
    rig.stop();
}

#[test]
fn a_non_canonical_repo_id_path_says_so() {
    let rig = default_rule_rig();
    send(
        &rig,
        "GET",
        "/repositories/101/../900",
        Some("token gho_moor-proxy"),
    );
    let why = reason(&only_withheld(&rig));
    assert_eq!(why, "repository id path is not canonical");
    rig.stop();
}
