//! Tests for name lookups and live-box scans.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName, ProviderId};

use crate::testing::clock::FakeClock;

use super::Store;

/// Open a store in a temp dir.
fn open(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
    crate::store::db::testing::open_store(&dir.join("moor.db"), clock)
}

/// Insert a box by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
        spec: SpecFields::default(),
    };
    store.apply(Decision::insert(nt)).unwrap().box_id
}

/// Close a box.
fn close(store: &Store, id: BoxId) {
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close { reason: None })
                .session(SessionInput::End),
        )
        .unwrap();
}

#[test]
fn name_lookups_distinguish_open_from_latest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let name = BoxName::parse_allow_bare("t").unwrap();
    assert!(store.open_box_by_name(&name).unwrap().is_none());
    assert!(store.latest_box_by_name(&name).unwrap().is_none());

    let first = add(&store, "t");
    assert_eq!(store.open_box_by_name(&name).unwrap().unwrap().id, first);
    close(&store, first);
    assert!(store.open_box_by_name(&name).unwrap().is_none());
    let latest = store.latest_box_by_name(&name).unwrap().unwrap();
    assert_eq!((latest.id, latest.lifecycle), (first, BoxLifecycle::Closed));

    let second = add(&store, "t");
    assert_eq!(store.open_box_by_name(&name).unwrap().unwrap().id, second);
    assert_eq!(store.latest_box_by_name(&name).unwrap().unwrap().id, second);
}

#[test]
fn box_rows_live_skips_deleted_boxes() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let fields = HandsFields {
        provider: Field::Set(ProviderId::new("p")),
        ..HandsFields::default()
    };
    let live = add(&store, "live");
    let gone = add(&store, "gone");
    let _none = add(&store, "none");
    for id in [live, gone] {
        let d = Decision::new(id)
            .lifecycle(BoxLifecycleInput::Admitted)
            .hands_input(HandsInc::FIRST, HandsInput::Requested)
            .hands(HandsInc::FIRST, fields.clone());
        store.apply(d).unwrap();
    }
    let failed = HandsInput::AcquireFailed {
        attempted: None,
        cause: "x".into(),
    };
    store
        .apply(Decision::new(gone).hands_input(HandsInc::FIRST, failed))
        .unwrap();
    let rows = store.box_rows_live().unwrap();
    let got: Vec<_> = rows.iter().map(|(id, b)| (*id, b.state)).collect();
    assert_eq!(got, [(live, HandsState::Starting)]);
}

#[test]
fn last_event_id_is_zero_without_events_then_the_newest_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    assert_eq!(store.last_event_id().unwrap(), 0);
    add(&store, "alpha");
    let newest = store.events_after(0, 1000).unwrap().last().unwrap().row.id;
    assert!(newest > 0);
    assert_eq!(store.last_event_id().unwrap(), newest);
}
