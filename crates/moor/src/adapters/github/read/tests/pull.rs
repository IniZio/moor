//! Reading the pull request from a Box's branch: which one counts, and what its state is.

use moor_core::seam::forge::{BranchPull, Checks, Forge, ForgeError, ForgeRepo, Pull, PullState};
use serde_json::{json, Value};

use super::{ok, token, FakeApi};
use crate::adapters::github::GitHub;

/// A pull request node from `head` in state `state`, with check rollup `rollup`.
fn node(number: u64, head: &str, state: &str, draft: bool, rollup: Option<&str>) -> Value {
    let rollup = rollup.map(|s| json!({"state": s}));
    json!({
        "number": number, "state": state, "isDraft": draft, "reviewDecision": null,
        "headRepository": {"nameWithOwner": head},
        "commits": {"nodes": [{"commit": {"statusCheckRollup": rollup}}]},
    })
}

/// What the read makes of a reply whose pull requests are `nodes`, and the branch it asked for.
fn pull_from(reply: Value) -> (Result<BranchPull, ForgeError>, String) {
    let api = FakeApi::start(move |vars| {
        assert_eq!(vars["owner"], "o");
        assert_eq!(vars["name"], "n");
        ok(&reply)
    });
    let got = GitHub::at(&api.url).pull_of(&ForgeRepo::new("o", "n"), "moor/7-x", &token());
    (got, api.request())
}

/// A reply for a pushed branch whose pull requests are `nodes`.
fn nodes(nodes: &[Value]) -> Value {
    json!({"data": {"repository": {"ref": {"name": "moor/7-x"}, "pullRequests": {"nodes": nodes}}}})
}

#[test]
fn the_newest_open_pull_from_the_repo_itself_wins_over_forks_and_closed_ones() {
    let (got, request) = pull_from(nodes(&[
        node(9, "fork/n", "OPEN", false, Some("SUCCESS")),
        node(8, "o/n", "CLOSED", false, Some("FAILURE")),
        node(7, "O/N", "OPEN", true, Some("PENDING")),
    ]));
    let want = Pull {
        number: 7,
        state: PullState::Draft,
        checks: Some(Checks::Pending),
    };
    assert_eq!(got.unwrap().pull, Some(want));
    assert!(request.contains("refs/heads/moor/7-x"), "{request}");
}

#[test]
fn with_none_open_the_newest_is_shown_without_its_checks() {
    let (got, _) = pull_from(nodes(&[
        node(8, "o/n", "MERGED", false, Some("SUCCESS")),
        node(5, "o/n", "CLOSED", false, None),
    ]));
    let want = Pull {
        number: 8,
        state: PullState::Merged,
        checks: None,
    };
    assert_eq!(got.unwrap().pull, Some(want));
}

#[test]
fn a_pushed_branch_with_no_pull_yet_reads_as_pushed_and_none() {
    let got = pull_from(nodes(&[])).0.unwrap();
    assert_eq!((got.pushed, got.pull), (true, None));
}

#[test]
fn a_branch_not_pushed_yet_reads_as_not_pushed() {
    let reply = json!({"data": {"repository": {"ref": null, "pullRequests": {"nodes": []}}}});
    assert!(!pull_from(reply).0.unwrap().pushed);
}

#[test]
fn checks_the_token_may_not_read_are_unknown_not_a_failure() {
    let mut reply = nodes(&[node(4, "o/n", "OPEN", false, None)]);
    reply["data"]["repository"]["pullRequests"]["nodes"][0]["commits"] = Value::Null;
    reply["errors"] = json!([{"type": "FORBIDDEN"}]);
    let got = pull_from(reply).0.unwrap().pull.unwrap();
    assert_eq!((got.state, got.checks), (PullState::Open, None));
}

#[test]
fn a_repo_the_token_cannot_see_is_not_visible() {
    let reply = json!({"data": {"repository": null}, "errors": [{"type": "NOT_FOUND"}]});
    assert_eq!(pull_from(reply).0, Err(ForgeError::NotVisible));
}
