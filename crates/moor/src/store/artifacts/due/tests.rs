//! Tests for the durable artifact-collection cursor.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};
use moor_schema::tables::{BoxHands, BoxState, Events};
use sea_query::{Expr, ExprTrait as _, Query, Value};

use crate::store::db::DbExt as _;
use crate::store::Store;
use crate::testing::clock::FakeClock;

/// Open a store in `dir` with a fake clock.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    crate::store::db::testing::open_store(&dir.join("moor.db"), clock)
}

/// Insert a Box that is Ready with a handle and no effect in flight.
fn ready(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
        spec: SpecFields::default(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    update(store, id, &[(BoxState::State, "ready".into())]);
    store
        .writer()
        .run(
            Query::insert()
                .into_table(BoxHands::Table)
                .columns([
                    BoxHands::Box,
                    BoxHands::Inc,
                    BoxHands::Provider,
                    BoxHands::State,
                    BoxHands::StateAt,
                    BoxHands::CreatedAt,
                ])
                .values_panic([
                    id.get().into(),
                    1_i64.into(),
                    "p".into(),
                    "ready".into(),
                    0_i64.into(),
                    0_i64.into(),
                ]),
        )
        .unwrap();
    set_handle(store, id, Some("h"));
    id
}

/// Set the handle of the Box's hands.
fn set_handle(store: &Store, id: BoxId, handle: Option<&str>) {
    store
        .writer()
        .run(
            Query::update()
                .table(BoxHands::Table)
                .value(BoxHands::Handle, handle)
                .and_where(Expr::col(BoxHands::Box).eq(id.get())),
        )
        .unwrap();
}

/// Assign `sets` on the Box row.
fn update(store: &Store, id: BoxId, sets: &[(BoxState, Value)]) {
    let mut q = Query::update();
    q.table(BoxState::Table)
        .and_where(Expr::col(BoxState::Box).eq(id.get()));
    for (col, v) in sets {
        q.value(*col, Expr::val(v.clone()));
    }
    store.writer().run(&mut q).unwrap();
}

/// Insert a raw event row and return its id.
fn event(store: &Store, id: BoxId, kind: &str) -> i64 {
    let conn = store.writer();
    conn.row(
        Query::insert()
            .into_table(Events::Table)
            .columns([Events::Box, Events::Kind, Events::Payload, Events::Ts])
            .values_panic([id.get().into(), kind.into(), "{}".into(), 0_i64.into()])
            .returning_col(Events::Id),
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn due_after_idled_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    assert!(store.artifacts_due().unwrap().is_empty());
    let e = event(&store, t, "idled");
    assert_eq!(store.artifacts_due().unwrap(), vec![(t, e)]);
    assert_eq!(store.newest_turn_event(t).unwrap(), Some(e));
}

#[test]
fn turn_done_counts_and_other_kinds_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    event(&store, t, "busy");
    assert!(store.artifacts_due().unwrap().is_empty());
    assert_eq!(store.newest_turn_event(t).unwrap(), None);
    let e = event(&store, t, "turn_done");
    assert_eq!(store.artifacts_due().unwrap(), vec![(t, e)]);
}

#[test]
fn not_due_once_cursor_reaches_newest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    let e = event(&store, t, "idled");
    store.set_artifacts_through(t, e).unwrap();
    assert!(store.artifacts_due().unwrap().is_empty());
    let next = event(&store, t, "turn_done");
    assert_eq!(store.artifacts_due().unwrap(), vec![(t, next)]);
}

#[test]
fn due_lists_boxes_in_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = ready(&store, "a");
    let b = ready(&store, "b");
    let eb = event(&store, b, "idled");
    let ea = event(&store, a, "idled");
    assert_eq!(store.artifacts_due().unwrap(), vec![(a, ea), (b, eb)]);
}

#[test]
fn not_due_unless_ready_with_handle_and_no_effect_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    event(&store, t, "idled");
    update(&store, t, &[(BoxState::State, "sleeping".into())]);
    assert!(store.artifacts_due().unwrap().is_empty());
    update(&store, t, &[(BoxState::State, "ready".into())]);
    set_handle(&store, t, None);
    assert!(store.artifacts_due().unwrap().is_empty());
    set_handle(&store, t, Some("h"));
    update(&store, t, &[(BoxState::Effect, "launch".into())]);
    assert!(store.artifacts_due().unwrap().is_empty());
    update(&store, t, &[(BoxState::Effect, None::<String>.into())]);
    assert_eq!(store.artifacts_due().unwrap().len(), 1);
}

#[test]
fn cursor_never_moves_backwards() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = ready(&store, "a");
    let first = event(&store, t, "idled");
    let second = event(&store, t, "idled");
    store.set_artifacts_through(t, second).unwrap();
    store.set_artifacts_through(t, first).unwrap();
    assert!(store.artifacts_due().unwrap().is_empty());
}
