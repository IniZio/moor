//! The store's connections: readers that only read, and no store call on a tokio runtime thread.

use moor_core::BoxId;
use moor_schema::tables::Meta;
use sea_query::{Expr, ExprTrait as _, Query};

use super::open;
use crate::store::db::DbExt as _;

#[test]
fn a_reader_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let wrote = store.reader().unwrap().run(
        Query::insert()
            .into_table(Meta::Table)
            .columns([Meta::K, Meta::V])
            .values_panic(["reader".into(), "wrote".into()]),
    );
    assert!(wrote.is_err(), "a reader wrote a row");
    let rows: i64 = store
        .reader()
        .unwrap()
        .row(
            Query::select()
                .expr(Expr::col(Meta::K).count())
                .from(Meta::Table)
                .and_where(Expr::col(Meta::K).eq("reader")),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a store call ran on a tokio runtime thread")]
fn a_store_call_on_a_tokio_runtime_thread_panics() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async { drop(store.box_row(BoxId::new(1))) });
}
