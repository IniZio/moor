//! Tests for the node scheduler.

use super::{schedule, BoxSpec, Node, Online, Scope, Verdict};
use crate::name::NodeName;
use crate::user::UserId;

const ME: UserId = UserId::new(1);
const YOU: UserId = UserId::new(2);
const NOW: i64 = 11 * 3600;

fn name(s: &str) -> NodeName {
    NodeName::parse(s).unwrap()
}

fn node(n: &str, scope: Scope, labels: &[&str]) -> Node {
    Node {
        name: name(n),
        scope,
        runtime: "crabbox".to_owned(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        max_boxes: None,
        online: Online::Online,
        used: 0,
        waiting: 0,
    }
}

fn mine(n: &str, machine_id: i64, enrolled_at: i64) -> Node {
    node(
        n,
        Scope::Personal {
            owner: ME,
            machine_id,
            enrolled_at,
        },
        &["linux"],
    )
}

fn ci() -> Node {
    node("ci", Scope::Shared, &["linux"])
}

fn spec(labels: &[&str], node: Option<&str>) -> BoxSpec {
    BoxSpec {
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        node: node.map(name),
    }
}

fn run(nodes: &[Node], s: &BoxSpec) -> Verdict {
    schedule(nodes, ME, Some(&name("ci")), s, NOW)
}

fn run_without_shared(nodes: &[Node], s: &BoxSpec) -> Verdict {
    schedule(nodes, ME, None, s, NOW)
}

fn bound(node: &str, machine_id: Option<i64>) -> Verdict {
    Verdict::Bind {
        node: name(node),
        runtime: "crabbox".to_owned(),
        machine_id,
    }
}

fn fail(v: Verdict) -> String {
    match v {
        Verdict::Fail { reason } => reason,
        other => format!("not Fail: {other:?}"),
    }
}

#[test]
fn default_shared_node_when_no_personal_node() {
    assert_eq!(run(&[ci()], &spec(&[], None)), bound("ci", None));
}

#[test]
fn personal_node_wins_over_shared() {
    let nodes = [ci(), mine("hv", 7, 100)];
    assert_eq!(run(&nodes, &spec(&[], None)), bound("hv", Some(7)));
}

#[test]
fn first_personal_by_enrolled_at_then_machine_id() {
    let nodes = [
        mine("b", 3, 200),
        mine("a", 9, 100),
        mine("c", 2, 100),
        ci(),
    ];
    assert_eq!(run(&nodes, &spec(&[], None)), bound("c", Some(2)));
}

#[test]
fn other_users_nodes_are_never_chosen() {
    let theirs = node(
        "theirs",
        Scope::Personal {
            owner: YOU,
            machine_id: 1,
            enrolled_at: 1,
        },
        &["linux"],
    );
    assert_eq!(run(&[theirs, ci()], &spec(&[], None)), bound("ci", None));
}

#[test]
fn named_node_is_the_only_candidate() {
    let nodes = [mine("hv", 7, 100), ci()];
    assert_eq!(run(&nodes, &spec(&[], Some("ci"))), bound("ci", None));
}

#[test]
fn unknown_name_lists_yours_and_shared() {
    let nodes = [mine("hv", 7, 100), ci()];
    let r = fail(run(&nodes, &spec(&[], Some("nope"))));
    assert_eq!(r, "no node named nope: yours are hv; shared: ci");
}

#[test]
fn unknown_name_with_no_nodes_says_none() {
    let r = fail(run(&[], &spec(&[], Some("nope"))));
    assert_eq!(r, "no node named nope: yours are none; shared: none");
}

#[test]
fn another_users_node_reads_as_missing() {
    let theirs = node(
        "theirs",
        Scope::Personal {
            owner: YOU,
            machine_id: 1,
            enrolled_at: 1,
        },
        &[],
    );
    let r = fail(run(&[theirs, ci()], &spec(&[], Some("theirs"))));
    assert_eq!(r, "no node named theirs: yours are none; shared: ci");
}

#[test]
fn labels_filter_candidates() {
    let mut gpu = mine("gpu", 9, 200);
    gpu.labels.push("gpu".to_owned());
    let nodes = [mine("hv", 7, 100), gpu, ci()];
    assert_eq!(run(&nodes, &spec(&["gpu"], None)), bound("gpu", Some(9)));
}

#[test]
fn no_node_with_labels_fails_listing_each() {
    let nodes = [mine("hv", 7, 100), ci()];
    let r = fail(run(&nodes, &spec(&["gpu"], None)));
    assert_eq!(r, "no node has labels gpu: hv has linux; ci has linux");
}

#[test]
fn named_node_lacking_labels_fails() {
    let nodes = [mine("hv", 7, 100), ci()];
    let r = fail(run(&nodes, &spec(&["gpu"], Some("hv"))));
    assert_eq!(r, "no node has labels gpu: hv has linux");
}

#[test]
fn offline_first_node_fails_with_skip_hint() {
    let mut hv = mine("hv", 7, 100);
    hv.online = Online::Offline {
        since: NOW - 12 * 60,
    };
    let r = fail(run(&[hv, ci()], &spec(&[], None)));
    assert_eq!(
        r,
        "hv is offline for 12m: start moor-machine-agent on it, or start with --node ci"
    );
}

#[test]
fn offline_default_node_has_no_skip_hint() {
    let mut c = ci();
    c.online = Online::Offline { since: NOW - 3600 };
    let r = fail(run(&[c], &spec(&[], None)));
    assert_eq!(r, "ci is offline for 1h: wait for it to come back");
}

#[test]
fn offline_named_node_fails_without_falling_through() {
    let mut hv = mine("hv", 7, 100);
    hv.online = Online::Offline { since: NOW };
    let r = fail(run(&[hv, ci()], &spec(&[], Some("hv"))));
    assert!(r.starts_with("hv is offline for 0m"), "{r}");
}

#[test]
fn full_node_is_pending_with_queue_ahead() {
    let mut c = ci();
    c.max_boxes = Some(5);
    c.used = 5;
    c.waiting = 2;
    assert_eq!(
        run(&[c], &spec(&[], None)),
        Verdict::Pending {
            node: name("ci"),
            reason: "ci is full (5 of 5 Boxes); 2 Boxes ahead".to_owned(),
        }
    );
}

#[test]
fn full_node_with_empty_queue_says_next() {
    let mut c = ci();
    c.max_boxes = Some(1);
    c.used = 1;
    let reason = match run(&[c], &spec(&[], None)) {
        Verdict::Pending { reason, .. } => reason,
        other => format!("not Pending: {other:?}"),
    };
    assert_eq!(reason, "ci is full (1 of 1 Boxes); 0 Boxes ahead");
}

#[test]
fn full_personal_node_does_not_spill_to_shared() {
    let mut hv = mine("hv", 7, 100);
    hv.max_boxes = Some(1);
    hv.used = 1;
    let v = run(&[hv, ci()], &spec(&[], None));
    assert!(
        matches!(v, Verdict::Pending { ref node, .. } if node.as_str() == "hv"),
        "{v:?}"
    );
}

#[test]
fn node_under_its_limit_binds() {
    let mut c = ci();
    c.max_boxes = Some(5);
    c.used = 4;
    assert_eq!(run(&[c], &spec(&[], None)), bound("ci", None));
}

#[test]
fn no_candidates_at_all_fails() {
    let r = fail(run(&[], &spec(&[], None)));
    assert_eq!(
        r,
        "no node can host this Box: shared default `ci` does not exist"
    );
}

const ENROLL: &str =
    "no node can host this Box: enroll a machine with moor-machine-agent enroll --runtime <provider>";

#[test]
fn without_a_shared_node_the_first_personal_node_binds() {
    let nodes = [mine("late", 8, 300), mine("hv", 7, 100)];
    assert_eq!(
        run_without_shared(&nodes, &spec(&[], None)),
        bound("hv", Some(7))
    );
}

#[test]
fn without_any_node_the_reason_says_how_to_get_one() {
    assert_eq!(fail(run_without_shared(&[], &spec(&[], None))), ENROLL);
    assert_eq!(fail(run_without_shared(&[], &spec(&["gpu"], None))), ENROLL);
}

#[test]
fn without_a_shared_node_other_users_nodes_do_not_count() {
    let theirs = node(
        "theirs",
        Scope::Personal {
            owner: YOU,
            machine_id: 3,
            enrolled_at: 1,
        },
        &["linux"],
    );
    assert_eq!(
        fail(run_without_shared(&[theirs], &spec(&[], None))),
        ENROLL
    );
}

#[test]
fn without_a_shared_node_own_nodes_lacking_labels_list_them() {
    let r = fail(run_without_shared(
        &[mine("hv", 7, 100)],
        &spec(&["gpu"], None),
    ));
    assert_eq!(r, "no node has labels gpu: hv has linux");
}

#[test]
fn without_a_shared_node_an_unknown_name_says_shared_none() {
    let r = fail(run_without_shared(
        &[mine("hv", 7, 100)],
        &spec(&[], Some("nope")),
    ));
    assert_eq!(r, "no node named nope: yours are hv; shared: none");
}

#[test]
fn without_a_shared_node_an_offline_node_offers_no_other_node() {
    let mut hv = mine("hv", 7, 100);
    hv.online = Online::Offline {
        since: NOW - 12 * 60,
    };
    let r = fail(run_without_shared(&[hv], &spec(&[], None)));
    assert_eq!(r, "hv is offline for 12m: start moor-machine-agent on it");
}
