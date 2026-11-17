//! The node read: shared nodes from config, personal nodes from enrolled machines.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use moor_core::config::{EnrolledMachine, NodeConfig, BOXES_HOST};
use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::name::NodeName;
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::view::user::SharedMachineKey;
use moor_core::{BoxId, BoxName, ProviderId};

use crate::store::tests::{new_box, open};
use crate::store::Store;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn on_node(store: &Store, id: BoxId, node: &str) {
    store
        .set_box_node(id, NodeName::parse(node).ok().as_ref())
        .unwrap();
}

fn wait(store: &Store, name: &str, node: &str) {
    let id = new_box(store, name);
    let d = Decision::new(id).lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 });
    store.apply(d).unwrap();
    on_node(store, id, node);
}

fn hold(store: &Store, name: &str, node: &str, provider: &str) {
    let id = new_box(store, name);
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(
            HandsInc::FIRST,
            HandsFields {
                provider: Field::Set(ProviderId::new(provider)),
                ..HandsFields::default()
            },
        );
    store.apply(d).unwrap();
    on_node(store, id, node);
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
        .set_machine_links(
            &crate::testing::runner(),
            false,
            &[MachineLink {
                provides: None,
                machine_id: up,
                online: true,
                since: Ts::from_millis(5),
                version: None,
                protocol: None,
                max_boxes: Some(4),
                cache_gb: None,
                features: Vec::new(),
                mem_mib: None,
                cpus: None,
                reserve_mib: None,
                memory: None,
            }],
            store.now(),
        )
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
    hold(&store, "one", "fast", "static");
    hold(&store, "two", "mini", "cloudhypervisor@mini");
    for name in ["w1", "w2", "w3"] {
        wait(&store, name, "mini");
    }
    let shared = BTreeMap::from([(
        "fast".to_owned(),
        NodeConfig {
            runtime: "static".to_owned(),
            labels: vec!["big".to_owned()],
            max_boxes: NonZeroU32::new(2),
        },
    )]);

    let nodes = store.nodes(&shared).unwrap();

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

fn ready_for(store: &Store, name: &str, node: &str, user: UserId) -> BoxId {
    let id = store
        .apply(
            Decision::insert(NewBox {
                name: BoxName::parse_allow_bare(name).unwrap(),
                repo: None,
                spec: SpecFields::default(),
            })
            .requester(user),
        )
        .unwrap()
        .box_id;
    let admit = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(
            HandsInc::FIRST,
            HandsFields {
                provider: Field::Set(ProviderId::new("p")),
                ..HandsFields::default()
            },
        );
    store.apply(admit).unwrap();
    let ready = Decision::new(id).lifecycle(BoxLifecycleInput::Ready {
        setup: SetupOutcome::Ok,
    });
    store.apply(ready).unwrap();
    on_node(store, id, node);
    id
}

#[test]
fn each_owner_on_a_node_is_picked_once_and_a_box_setting_up_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let enroll = |who| {
        store
            .enroll(&Identity::parse(who).unwrap(), None)
            .unwrap()
            .0
    };
    let (alice, bob) = (enroll("authgear:a"), enroll("authgear:b"));
    let first = ready_for(&store, "a1", "hv", alice);
    ready_for(&store, "a2", "hv", alice);
    let bobs = ready_for(&store, "b1", "hv", bob);
    ready_for(&store, "elsewhere", "other", bob);
    hold(&store, "starting", "hv", "p");

    let node = NodeName::parse("hv").unwrap();
    assert_eq!(
        store.one_live_per_owner_on(&node).unwrap(),
        vec![first, bobs]
    );
}

const OTHER: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMKNmEuquBc3TqUA/DQWHHNt+ujMoN27+qMGjIZmpouo";

#[test]
fn a_shared_machine_is_a_shared_node_with_its_machine_and_its_key_is_listed_apart() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    alice_with_machines(&store);
    let local = EnrolledMachine::parse("local", KEY, &[], 1)
        .unwrap()
        .with_pool(BOXES_HOST)
        .unwrap();

    let id = store.enroll_shared_machine(&local).unwrap();

    let nodes = store.nodes(&BTreeMap::new()).unwrap();
    let node = find(&nodes, "local");
    assert_eq!((&node.scope, node.machine), (&Scope::Shared, Some(id)));
    assert_eq!(node.runtime, BOXES_HOST);
    assert!(matches!(node.online, Online::Offline { .. }));
    assert_eq!(
        store.shared_machine_keys().unwrap(),
        [SharedMachineKey {
            machine_id: id,
            name: "local".to_owned(),
            pubkey: KEY.to_owned(),
        }]
    );
    let owned = store.machine_keys().unwrap();
    assert!(owned.iter().all(|k| k.machine_id != id));
    assert_eq!(owned.len(), 3, "alice's machines are listed as before");
}

#[test]
fn enrolling_a_shared_machine_again_keeps_its_id_and_replaces_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let first = EnrolledMachine::parse("local", KEY, &[], 1).unwrap();
    let again = EnrolledMachine::parse("local", OTHER, &["a".to_owned()], 1).unwrap();
    let id = store.enroll_shared_machine(&first).unwrap();

    assert_eq!(store.enroll_shared_machine(&again).unwrap(), id);

    let keys = store.shared_machine_keys().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys.first().unwrap().pubkey, OTHER);
}

#[test]
fn a_user_machine_of_the_same_name_is_not_a_shared_one() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = alice_with_machines(&store);
    let local = EnrolledMachine::parse("local", KEY, &[], 1).unwrap();
    store.enroll_shared_machine(&local).unwrap();

    assert!(store.user_machine_named("mini").unwrap());
    assert!(!store.user_machine_named("local").unwrap());
    let mine = store.user_machines(alice).unwrap();
    assert!(mine.iter().all(|m| m.name != "local"));
}

#[test]
fn a_node_carries_how_its_agent_provides_boxes_until_a_link_says_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let local = EnrolledMachine::parse("local", KEY, &[], 1)
        .unwrap()
        .with_pool(BOXES_HOST)
        .unwrap();
    let id = store.enroll_shared_machine(&local).unwrap();
    let link = |provides: Option<&str>| MachineLink {
        provides: provides.map(str::to_owned),
        machine_id: id,
        online: true,
        since: Ts::from_millis(5),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        memory: None,
    };
    let provides = |store: &Store| {
        let nodes = store.nodes(&BTreeMap::new()).unwrap();
        find(&nodes, "local").provides.clone()
    };
    assert_eq!(provides(&store), None, "before any link");
    for sent in [Some("docker"), None] {
        let runner = crate::testing::runner();
        let links = [link(sent)];
        store
            .set_machine_links(&runner, false, &links, store.now())
            .unwrap();
        assert_eq!(provides(&store).as_deref(), sent);
    }
}
