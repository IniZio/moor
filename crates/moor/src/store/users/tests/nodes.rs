//! The node read: shared nodes from config, personal nodes from enrolled machines.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;

use moor_core::config::{EnrolledMachine, NodeConfig};
use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::ProviderId;

use crate::store::tests::{new_box, open};
use crate::store::Store;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn hold(store: &Store, name: &str, provider: &str) {
    let id = new_box(store, name);
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new(provider)),
            ..Fields::default()
        });
    store.apply(d).unwrap();
}

fn pool(store: &Store, user: UserId, name: &str, runtime: &str) -> i64 {
    let mut m = EnrolledMachine::parse(name, KEY, &["gpu".to_owned()], 1).unwrap();
    m.pool = Some(runtime.to_owned());
    store.enroll_machine(user, &m).unwrap()
}

fn find<'a>(nodes: &'a [Node], name: &str) -> &'a Node {
    nodes.iter().find(|n| n.name.as_str() == name).unwrap()
}

/// Alice's online `mini` (cap 4), her offline `lost`, and a machine that offers no runtime.
fn alice_with_machines(store: &Store) -> UserId {
    let (alice, _) = store
        .enroll(&Identity::parse("authgear:a").unwrap(), None)
        .unwrap();
    let up = pool(store, alice, "mini", "cloudhypervisor");
    pool(store, alice, "lost", "cloudhypervisor");
    let plain = EnrolledMachine::parse("plain", KEY, &[], 1).unwrap();
    store.enroll_machine(alice, &plain).unwrap();
    store
        .set_machine_links(&[MachineLink {
            machine_id: up,
            online: true,
            since: Ts::from_millis(5),
            version: None,
            protocol: None,
            max_boxes: Some(4),
            cache_gb: None,
            features: Vec::new(),
        }])
        .unwrap();
    alice
}

fn check_mini(nodes: &[Node], alice: UserId) {
    let mini = find(nodes, "mini");
    assert!(matches!(mini.scope, Scope::Personal { owner, .. } if owner == alice));
    assert_eq!(
        (mini.online, mini.used, mini.waiting, mini.max_boxes),
        (Online::Online, 1, 3, Some(4))
    );
    assert_eq!(mini.labels, vec!["gpu".to_owned()]);
}

#[test]
fn shared_and_personal_nodes_carry_their_link_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = alice_with_machines(&store);
    hold(&store, "one", "static");
    hold(&store, "two", "cloudhypervisor@mini");
    let shared = BTreeMap::from([(
        "fast".to_owned(),
        NodeConfig {
            runtime: "static".to_owned(),
            labels: vec!["big".to_owned()],
            max_boxes: NonZeroU32::new(2),
        },
    )]);
    let waiting = HashMap::from([("cloudhypervisor@mini".to_owned(), 3)]);

    let nodes = store.nodes(&shared, &waiting).unwrap();

    assert_eq!(nodes.len(), 3, "a machine without a runtime is no node");
    let fast = find(&nodes, "fast");
    assert_eq!(fast.scope, Scope::Shared);
    assert_eq!(
        (fast.online, fast.used, fast.max_boxes),
        (Online::Online, 1, Some(2))
    );
    check_mini(&nodes, alice);
    let lost = find(&nodes, "lost");
    assert_eq!(lost.online, Online::Offline { since: 1000 });
    assert_eq!((lost.used, lost.waiting), (0, 0));
}
