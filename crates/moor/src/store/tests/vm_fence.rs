//! A write about a Box's VM names the hands it was made for. Once the Box has newer hands the
//! write is refused and changes nothing; a write that names no hands, as an older binary sends,
//! still lands.

use moor_core::config::UserMachine;
use moor_core::domain::hands::HandsInc;
use moor_core::domain::layers::LayerKey;
use moor_core::request::{BoxHostMemory, BoxStage, MachineBox};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::{Identity, UserId};
use moor_core::{BoxHandle, BoxId, ProviderId};
use moor_schema::tables::SetupCheckpointPins as Pins;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::{new_box, open};
use crate::store::db::DbExt as _;
use crate::store::{Fence, Store, StoreError};
use crate::testing::rng::FakeRng;

const OLD: HandsInc = HandsInc::FIRST;
const NEW: HandsInc = HandsInc::new(2);

/// A Box whose first hands were replaced by second ones.
fn replaced(store: &Store, name: &str) -> BoxId {
    let id = new_box(store, name);
    store.replace_hands(id);
    id
}

fn is_stale(got: &Result<(), StoreError>, id: BoxId) -> bool {
    matches!(got, Err(StoreError::Stale { box_id, read }) if *box_id == id && *read == Fence::Hands(OLD))
}

fn spec(id: BoxId, handle: &str, inc: Option<HandsInc>) -> ProxySpec {
    ProxySpec {
        box_id: id,
        box_handle: Some(BoxHandle::new(handle)),
        box_repo: None,
        other_repos: Vec::new(),
        rules: Vec::new(),
        user: UserId::new(1),
        hands_inc: inc.map(HandsInc::get),
    }
}

#[test]
fn a_proxy_spec_for_replaced_hands_is_refused_and_the_newer_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = replaced(&store, "a");
    store.reserve(id, &FakeRng::default()).unwrap();
    let newer = spec(id, "h2", Some(NEW));
    store.set_proxy_spec(id, &newer).unwrap();

    assert!(is_stale(
        &store.set_proxy_spec(id, &spec(id, "h1", Some(OLD))),
        id
    ));
    assert_eq!(store.proxy_spec(id).unwrap(), Some(newer));
}

#[test]
fn a_proxy_spec_naming_no_hands_still_lands() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = replaced(&store, "a");
    store.reserve(id, &FakeRng::default()).unwrap();
    let unnamed = spec(id, "h2", None);
    store.set_proxy_spec(id, &unnamed).unwrap();
    assert_eq!(store.proxy_spec(id).unwrap(), Some(unnamed));
}

/// An enrolled machine's id.
fn machine(store: &Store) -> i64 {
    let (user, _) = store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    let mini = UserMachine::parse("mini", &[]).unwrap();
    store.set_user_machine(user, &mini).unwrap()
}

fn record(id: BoxId, machine_id: i64, slot: u16, inc: Option<HandsInc>) -> MachineBox {
    MachineBox {
        box_id: id,
        machine_id,
        slot,
        reach: "{}".to_owned(),
        facts: "{}".to_owned(),
        stage: BoxStage::Prepared,
        evicted: false,
        inc: inc.map(HandsInc::get),
    }
}

#[test]
fn a_runner_record_for_replaced_hands_is_refused_and_one_naming_none_lands() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let m = machine(&store);
    let id = replaced(&store, "a");
    store.put_machine_box(&record(id, m, 1, Some(NEW))).unwrap();

    assert!(is_stale(
        &store.put_machine_box(&record(id, m, 7, Some(OLD))),
        id
    ));
    assert_eq!(store.machine_box(id).unwrap().map(|r| r.slot), Some(1));

    store.put_machine_box(&record(id, m, 3, None)).unwrap();
    assert_eq!(store.machine_box(id).unwrap().map(|r| r.slot), Some(3));
}

#[test]
fn memory_reported_for_replaced_hands_is_dropped_and_returned_as_stale() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let m = machine(&store);
    let (late, unnamed, current) = (
        replaced(&store, "a"),
        replaced(&store, "b"),
        replaced(&store, "c"),
    );
    let on = |box_id, mib, inc: Option<HandsInc>| BoxHostMemory {
        machine_id: m,
        controller: store.controller_id().unwrap(),
        box_id,
        host_memory_mib: mib,
        resume_memory_mib: None,
        inc: inc.map(HandsInc::get),
    };

    let stale = store
        .set_box_memory(&[
            on(late, 100, Some(OLD)),
            on(unnamed, 200, None),
            on(current, 300, Some(NEW)),
        ])
        .unwrap();

    assert!(
        matches!(stale.as_slice(), [StoreError::Stale { box_id, read }] if *box_id == late && *read == Fence::Hands(OLD)),
        "{stale:?}"
    );
    let held = |id| store.box_host_memory(id).unwrap();
    assert_eq!(
        [held(late), held(unnamed), held(current)],
        [None, Some(200), Some(300)]
    );
}

/// The keys of the layers Box `id` stands on, by key.
fn pinned(store: &Store, id: BoxId) -> Vec<String> {
    let rows = store
        .writer()
        .rows(
            Query::select()
                .column(Pins::LayerKey)
                .from(Pins::Table)
                .and_where(Expr::col(Pins::BoxId).eq(id.get()))
                .order_by(Pins::LayerKey, Order::Asc),
            |r| r.get(0),
        )
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

#[test]
fn a_layer_saved_on_replaced_hands_pins_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = replaced(&store, "a");
    let provider = ProviderId::new("p");
    let key = |c: &str| LayerKey::restore(c.repeat(64)).unwrap();

    assert!(is_stale(
        &store.pin_checkpoint((id, Some(OLD)), &provider, &key("a"), 1),
        id
    ));
    store
        .pin_checkpoint((id, Some(NEW)), &provider, &key("b"), 1)
        .unwrap();
    store
        .pin_checkpoint((id, None), &provider, &key("c"), 0)
        .unwrap();

    assert_eq!(pinned(&store, id), ["b".repeat(64), "c".repeat(64)]);
}
