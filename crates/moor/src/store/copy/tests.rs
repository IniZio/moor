//! Tests for copying the `SQLite` store into Postgres.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::HookCache;
use sea_query::{Expr, ExprTrait as _, Query};

use super::copy_to_postgres;
use crate::store::db::testing::postgres_target;
use crate::store::db::{Conn, DbExt as _, Target};
use crate::store::tests::new_box;
use crate::store::Store;
use crate::testing::clock::FakeClock;

fn clock() -> Arc<FakeClock> {
    Arc::new(FakeClock::new(Ts::from_millis(1_000_000)))
}

/// A `SQLite` store at `path`, with two Boxes, refs and bindings added in an order a sorted read
/// would not give back.
fn filled(path: &Path) -> (Store, [BoxId; 2]) {
    let store = Store::open(path, clock()).unwrap();
    let first = new_box(&store, "acme/1");
    let second = new_box(&store, "acme/2");
    store
        .apply(Decision::new(second).lifecycle(BoxLifecycleInput::Admitted))
        .unwrap();
    store.add_box_ref(first, "pr", "acme/app#9").unwrap();
    store.add_box_ref(second, "pr", "acme/app#2").unwrap();
    store.add_box_ref(first, "branch", "feat/x").unwrap();
    store.add_box_ref(first, "pr", "acme/app#3").unwrap();
    let one = store.open_host_session("/a", None, None).unwrap();
    store.bind_session("cli", "zz", one, "U1").unwrap();
    store.bind_session("cli", "aa", one, "U2").unwrap();
    store.end_host_session(one).unwrap();
    let two = store.open_host_session("/b", None, None).unwrap();
    store.bind_session("cli", "zz", two, "U3").unwrap();
    (store, [first, second])
}

/// The Postgres database the tests run on, none when `MOOR_TEST_POSTGRES_URL` is not set.
fn target(dir: &Path) -> Option<Target> {
    postgres_target(&dir.join("target.db"))
}

/// A filled `SQLite` store and its copy on Postgres, none when no Postgres is configured.
fn copied(dir: &Path) -> Option<(Store, Store, [BoxId; 2])> {
    let path = dir.join("moor.db");
    let (source, ids) = filled(&path);
    let target = target(dir)?;
    let counts = copy_to_postgres(&path, &target).unwrap();
    assert!(counts.iter().any(|c| c.table == "events" && c.rows > 0));
    assert!(counts.iter().any(|c| c.table == "bindings" && c.rows == 3));
    let copy = Store::open_postgres(&target, dir, clock()).unwrap();
    Some((source, copy, ids))
}

#[test]
fn a_copy_reads_back_boxes_and_events_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let Some((source, copy, ids)) = copied(dir.path()) else {
        return;
    };
    for id in ids {
        assert_eq!(copy.box_row(id).unwrap(), source.box_row(id).unwrap());
        assert_eq!(
            copy.events(id, 100).unwrap(),
            source.events(id, 100).unwrap()
        );
    }
}

#[test]
fn a_copy_keeps_the_order_refs_and_bindings_were_added_in() {
    let dir = tempfile::tempdir().unwrap();
    let Some((source, copy, [first, _])) = copied(dir.path()) else {
        return;
    };
    assert_eq!(
        copy.box_refs(first, "pr").unwrap(),
        ["acme/app#9", "acme/app#3"]
    );
    assert_eq!(
        copy.box_refs(first, "pr").unwrap(),
        source.box_refs(first, "pr").unwrap()
    );
    for key in ["zz", "aa"] {
        assert_eq!(
            copy.binding("cli", key).unwrap(),
            source.binding("cli", key).unwrap()
        );
    }
    let pr = "acme/app#2";
    assert_eq!(
        copy.boxes_by_ref("pr", pr).unwrap(),
        source.boxes_by_ref("pr", pr).unwrap()
    );
}

#[test]
fn a_copied_store_hands_out_ids_past_the_copied_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let (_, [_, second]) = filled(&path);
    let Some(target) = target(dir.path()) else {
        return;
    };
    copy_to_postgres(&path, &target).unwrap();
    let copy = Store::open_postgres(&target, dir.path(), clock()).unwrap();
    let newest = new_box(&copy, "acme/3");
    assert!(newest.get() > second.get(), "a new Box takes a fresh id");
    copy.add_box_ref(newest, "pr", "acme/app#1").unwrap();
    let after: i64 = copy
        .reader()
        .unwrap()
        .row(
            Query::select().expr(Expr::col("id").max()).from("box_refs"),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, 5, "a new ref continues past the copied ones");
}

#[test]
fn a_target_that_holds_rows_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    drop(filled(&path));
    let Some(target) = target(dir.path()) else {
        return;
    };
    let held = Store::open_postgres(&target, dir.path(), clock()).unwrap();
    let id = new_box(&held, "other/1");
    let err = copy_to_postgres(&path, &target).unwrap_err().to_string();
    assert!(err.contains("already holds rows in"), "{err}");
    assert_eq!(held.open_boxes().unwrap().len(), 1);
    assert!(held.box_row(id).unwrap().is_some());
}

#[test]
fn a_migrated_target_with_no_rows_is_copied_into() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let (source, [first, _]) = filled(&path);
    let Some(target) = target(dir.path()) else {
        return;
    };
    drop(Store::open_postgres(&target, dir.path(), clock()).unwrap());
    copy_to_postgres(&path, &target).unwrap();
    let copy = Store::open_postgres(&target, dir.path(), clock()).unwrap();
    assert_eq!(copy.box_row(first).unwrap(), source.box_row(first).unwrap());
}

#[test]
fn a_source_not_at_this_builds_latest_migration_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    drop(filled(&path));
    let conn = Conn::open_file(&path).unwrap();
    conn.run(
        Query::delete()
            .from_table("refinery_schema_history")
            .and_where(Expr::col("version").eq(46)),
    )
    .unwrap();
    drop(conn);
    let target = Target::parse("postgres://moor@127.0.0.1:1/none").unwrap();
    let err = copy_to_postgres(&path, &target).unwrap_err().to_string();
    assert!(err.contains("migration 45"), "{err}");
    assert!(err.contains("moor store migrate"), "{err}");
}

#[test]
fn a_missing_source_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = Target::parse("postgres://moor@127.0.0.1:1/none").unwrap();
    assert!(copy_to_postgres(&dir.path().join("moor.db"), &target).is_err());
}

#[test]
fn a_table_with_more_rows_than_one_statement_can_bind_is_copied_whole() {
    const ROWS: i64 = 25_000;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let source = Store::open(&path, clock()).unwrap();
    let sessions: Vec<i64> = (0..ROWS)
        .map(|_| source.open_host_session("/a", None, None).unwrap())
        .collect();
    for chunk in sessions.chunks(5_000) {
        let mut insert = Query::insert();
        insert.into_table(HookCache::Table).columns([
            HookCache::Session,
            HookCache::Record,
            HookCache::At,
        ]);
        for session in chunk {
            insert.values_panic([(*session).into(), "{}".into(), 1.into()]);
        }
        source.writer().run(insert).unwrap();
    }
    let Some(target) = target(dir.path()) else {
        return;
    };
    let counts = copy_to_postgres(&path, &target).unwrap();
    let hooks = counts.iter().find(|c| c.table == "hook_cache").unwrap();
    assert_eq!(hooks.rows, ROWS);
}

#[test]
fn a_batch_of_the_widest_table_stays_under_the_bind_limit() {
    let widest = moor_schema::tables::all()
        .iter()
        .map(|(_, columns)| columns.len())
        .max()
        .unwrap();
    assert!(super::BATCH * widest < 65_535, "{widest} columns");
}
