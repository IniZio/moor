//! Which Boxes hold a slot on a node.

use moor_core::decide::schedule::BoxSpec;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::name::NodeName;
use moor_core::BoxHandle;

use super::{live_on_node_in, new_box, node_p, open, provider};

#[test]
fn a_node_counts_its_starting_and_ready_boxes_only() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let count = |node: &NodeName| live_on_node_in(&store.reader().unwrap(), node).unwrap();
    assert_eq!(count(&node_p()), 0);

    let [starting, ready, failed, released, other] =
        ["a", "b", "c", "d", "e"].map(|n| new_box(&store, n));
    for id in [starting, ready, failed, released, other] {
        let d = Decision::new(id)
            .lifecycle(BoxLifecycleInput::Admitted)
            .box_input(BoxInput::Requested)
            .set(provider());
        store.apply(d).unwrap();
        let node = if id == other { "q" } else { "p" };
        let spec = BoxSpec {
            labels: Vec::new(),
            node: NodeName::parse(node).ok(),
        };
        store.set_box_spec(id, &spec).unwrap();
    }
    let input = |id, input| store.apply(Decision::new(id).box_input(input)).unwrap();
    let acquired = || BoxInput::Acquired {
        handle: BoxHandle::new("h"),
    };
    for id in [ready, released, other] {
        input(id, acquired());
    }
    input(released, BoxInput::Released);
    let cause = "x".into();
    input(
        failed,
        BoxInput::AcquireFailed {
            attempted: None,
            cause,
        },
    );

    assert_eq!(count(&node_p()), 2);
    assert_eq!(count(&NodeName::parse("q").unwrap()), 1);
}
