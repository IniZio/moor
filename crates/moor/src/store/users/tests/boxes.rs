//! The runner's Box records: written whole, read by Box or by machine, dropped on close.

use moor_core::config::EnrolledMachine;
use moor_core::name::BoxId;
use moor_core::request::{BoxStage, MachineBox};
use moor_core::user::Identity;

use crate::store::tests::open;
use crate::store::Store;

const KEYS: [&str; 2] = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
];

fn two_machines(store: &Store) -> [i64; 2] {
    let (alice, _) = store
        .enroll(&Identity::parse("authgear:a").unwrap(), None)
        .unwrap();
    let [a, b] = KEYS;
    [("one", a), ("two", b)].map(|(name, key)| {
        let m = EnrolledMachine::parse(name, key, &[], 1).unwrap();
        store.enroll_machine(alice, &m).unwrap()
    })
}

fn record(box_id: i64, machine_id: i64, stage: BoxStage) -> MachineBox {
    MachineBox {
        box_id: BoxId::new(box_id),
        machine_id,
        slot: 2,
        reach: format!(r#"{{"machine_id":{machine_id},"machine":"m"}}"#),
        facts: r#"{"device":"d"}"#.to_owned(),
        stage,
        evicted: false,
    }
}

#[test]
fn a_record_reads_back_as_written_and_a_put_replaces_it_whole() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let [one, two] = two_machines(&store);
    assert_eq!(store.machine_box(BoxId::new(7)).unwrap(), None);
    let prepared = record(7, one, BoxStage::Prepared);
    store.put_machine_box(&prepared).unwrap();
    assert_eq!(store.machine_box(BoxId::new(7)).unwrap(), Some(prepared));
    let moved = MachineBox {
        evicted: true,
        ..record(7, two, BoxStage::Suspended { at: 1_700_000_000 })
    };
    store.put_machine_box(&moved).unwrap();
    assert_eq!(store.machine_box(BoxId::new(7)).unwrap(), Some(moved));
    let noted = record(7, two, BoxStage::Noted);
    store.put_machine_box(&noted).unwrap();
    assert_eq!(store.machine_box(BoxId::new(7)).unwrap(), Some(noted));
}

#[test]
fn a_machine_lists_only_its_own_records_and_a_drop_removes_one() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let [one, two] = two_machines(&store);
    for (id, machine) in [(9, one), (7, one), (8, two)] {
        store
            .put_machine_box(&record(id, machine, BoxStage::Prepared))
            .unwrap();
    }
    let ids = |machine| -> Vec<i64> {
        let rows = store.machine_boxes(machine).unwrap();
        rows.iter().map(|r| r.box_id.get()).collect()
    };
    assert_eq!(ids(one), [7, 9]);
    assert_eq!(ids(two), [8]);
    store.drop_machine_box(BoxId::new(7)).unwrap();
    store.drop_machine_box(BoxId::new(7)).unwrap();
    assert_eq!(ids(one), [9]);
    assert_eq!(store.machine_box(BoxId::new(7)).unwrap(), None);
}
