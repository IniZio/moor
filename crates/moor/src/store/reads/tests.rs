//! Tests for name lookups and live-box scans.

use moor_core::decide::turn::SteerMark;
use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName, ProviderId};
use moor_schema::tables::Events;
use sea_query::Query;

use crate::store::db::DbExt as _;
use crate::testing::clock::FakeClock;

use super::Store;

/// Open a store in a temp dir.
fn open(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a box by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
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
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    let live = add(&store, "live");
    let gone = add(&store, "gone");
    let _none = add(&store, "none");
    for id in [live, gone] {
        let d = Decision::new(id)
            .lifecycle(BoxLifecycleInput::Admitted)
            .box_input(BoxInput::Requested)
            .set(fields.clone());
        store.apply(d).unwrap();
    }
    let failed = BoxInput::AcquireFailed {
        attempted: None,
        cause: "x".into(),
    };
    store.apply(Decision::new(gone).box_input(failed)).unwrap();
    let rows = store.box_rows_live().unwrap();
    let got: Vec<_> = rows.iter().map(|(id, b)| (*id, b.state)).collect();
    assert_eq!(got, [(live, BoxState::Starting)]);
}

fn event(store: &Store, id: BoxId, kind: &str, payload: &str, ts: i64) -> i64 {
    let conn = store.writer();
    conn.row(
        Query::insert()
            .into_table(Events::Table)
            .columns([Events::Box, Events::Kind, Events::Payload, Events::Ts])
            .values_panic([id.get().into(), kind.into(), payload.into(), ts.into()])
            .returning_col(Events::Id),
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn unanswered_steer_is_the_newest_steer_without_turn_done() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add(&store, "a");
    event(&store, id, "steered", "{}", 10);
    let newest = event(&store, id, "steered", "{}", 20);
    assert_eq!(
        store.unanswered_steer(id).unwrap(),
        Some(SteerMark {
            id: newest,
            hook_before: None,
            new_turn: false,
        })
    );
}

#[test]
fn unanswered_steer_reads_the_hook_time_before_the_send() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add(&store, "a");
    let steer = event(
        &store,
        id,
        "steered",
        r#"{"hook_before":1234,"new_turn":true}"#,
        10,
    );
    assert_eq!(
        store.unanswered_steer(id).unwrap(),
        Some(SteerMark {
            id: steer,
            hook_before: Some(Ts::from_millis(1234)),
            new_turn: true,
        })
    );
}

#[test]
fn answered_newest_steer_yields_none_even_with_older_unanswered() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add(&store, "a");
    event(&store, id, "steered", "{}", 10);
    let newest = event(&store, id, "steered", "{}", 20);
    event(
        &store,
        id,
        "turn_done",
        &format!("{{\"steer\":{newest}}}"),
        30,
    );
    assert_eq!(store.unanswered_steer(id).unwrap(), None);
}

#[test]
fn no_steer_yields_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add(&store, "a");
    event(&store, id, "turn_done", "{\"steer\":1}", 30);
    assert_eq!(store.unanswered_steer(id).unwrap(), None);
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
