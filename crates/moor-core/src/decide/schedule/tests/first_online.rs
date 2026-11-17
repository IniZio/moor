//! Tests for a start that takes the first online node.

use super::{
    bound, ci, fail, mine, name, run, spec, BoxSpec, Node, Online, Placement, Verdict, NOW,
};

fn firing() -> BoxSpec {
    BoxSpec {
        placement: Placement::FirstOnline,
        ..spec(&[], None)
    }
}

fn offline(n: Node) -> Node {
    Node {
        online: Online::Offline { since: NOW - 600 },
        ..n
    }
}

#[test]
fn a_firing_skips_an_offline_personal_node_for_the_online_shared_one() {
    let nodes = [offline(mine("laptop", 1, 10)), ci()];
    assert_eq!(run(&nodes, &firing()), bound("ci", None));
}

#[test]
fn a_manual_start_refuses_in_the_same_state() {
    let nodes = [offline(mine("laptop", 1, 10)), ci()];
    let reason = fail(run(&nodes, &spec(&[], None)));
    assert!(reason.contains("laptop is offline"), "{reason}");
    assert!(reason.contains("--node ci"), "{reason}");
}

#[test]
fn a_firing_still_refuses_when_nothing_is_online() {
    let nodes = [offline(mine("laptop", 1, 10)), offline(ci())];
    let reason = fail(run(&nodes, &firing()));
    assert!(reason.contains("laptop is offline"), "{reason}");
}

#[test]
fn a_firing_queues_on_a_full_online_node() {
    let full = Node {
        max_boxes: Some(1),
        used: 1,
        ..ci()
    };
    let nodes = [offline(mine("laptop", 1, 10)), full];
    assert!(matches!(
        run(&nodes, &firing()),
        Verdict::Pending { node, .. } if node == name("ci")
    ));
}

#[test]
fn a_firing_keeps_an_online_personal_node_first() {
    let nodes = [mine("laptop", 1, 10), ci()];
    assert_eq!(run(&nodes, &firing()), bound("laptop", Some(1)));
}
