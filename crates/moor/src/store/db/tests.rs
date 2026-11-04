use sea_query::{ColumnDef, Expr, ExprTrait as _, Func, Index, OnConflict, Order, Query, Table};

use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_migrate::Outcome;

use super::testing::{open_conn, open_store};
use super::{Conn, Db, DbExt as _, Error, Kind, OptionalExtension as _, UniqueKey};
use crate::testing::clock::FakeClock;

type Read = (i64, bool, Option<String>, Option<Vec<u8>>);

/// A connection to an empty database holding table `t`, kept alive by the returned dir.
fn conn() -> (Conn, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_conn(&dir.path().join("t.db"));
    conn.schema(
        Table::create()
            .table("t")
            .col(
                ColumnDef::new("id")
                    .big_integer()
                    .primary_key()
                    .auto_increment(),
            )
            .col(ColumnDef::new("n").big_integer())
            .col(ColumnDef::new("s").text())
            .col(ColumnDef::new("b").blob()),
    )
    .unwrap();
    conn.schema(Index::create().name("t_s").table("t").col("s").unique())
        .unwrap();
    (conn, dir)
}

fn insert_n(conn: &impl Db, n: i64) {
    conn.run(
        Query::insert()
            .into_table("t")
            .columns(["n"])
            .values_panic([n.into()]),
    )
    .unwrap();
}

fn all_n(conn: &impl Db) -> Vec<i64> {
    conn.rows(
        Query::select()
            .column("n")
            .from("t")
            .order_by("n", Order::Asc),
        |r| r.get(0),
    )
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn a_column_of_the_wrong_type_is_refused_with_its_name() {
    let (conn, _dir) = conn();
    conn.run(
        Query::insert()
            .into_table("t")
            .columns(["s"])
            .values_panic(["x".into()]),
    )
    .unwrap();
    let err = conn
        .row(Query::select().column("s").from("t"), |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_err();
    assert!(
        matches!(&err, Error::Type { index: 0, name, found: Kind::Text } if name == "s"),
        "{err:?}"
    );
    let err = conn
        .row(Query::select().expr_as(Expr::val(-1), "n"), |r| {
            r.get::<_, u32>("n")
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::OutOfRange {
                index: 0,
                value: -1
            }
        ),
        "{err:?}"
    );
}

#[test]
fn no_row_is_an_error_that_optional_turns_into_none() {
    let (conn, _dir) = conn();
    let none = conn
        .row(Query::select().column("n").from("t"), |r| {
            r.get::<_, i64>(0)
        })
        .optional()
        .unwrap();
    assert_eq!(none, None);
}

#[test]
fn rows_are_mapped_only_as_they_are_taken() {
    let (conn, _dir) = conn();
    insert_n(&conn, 1);
    insert_n(&conn, 2);
    let mut rows = conn
        .rows(
            Query::select()
                .column("n")
                .from("t")
                .order_by("n", Order::Asc),
            |r| match r.get::<_, i64>(0)? {
                1 => Ok(1),
                n => Err(Error::OutOfRange { index: 0, value: n }),
            },
        )
        .unwrap();
    assert_eq!(rows.next().transpose().unwrap(), Some(1));
}

#[test]
fn a_dropped_transaction_rolls_back_and_a_committed_one_stays() {
    let (mut conn, _dir) = conn();
    {
        let tx = conn.immediate().unwrap();
        insert_n(&tx, 1);
    }
    let tx = conn.transaction().unwrap();
    insert_n(&tx, 2);
    tx.commit().unwrap();
    assert_eq!(all_n(&conn), [2]);
}

#[test]
fn a_unique_collision_names_its_table_and_columns() {
    let (conn, _dir) = conn();
    let put = |id: i64, s: &str| {
        conn.run(
            Query::insert()
                .into_table("t")
                .columns(["id", "s"])
                .values_panic([id.into(), s.into()]),
        )
    };
    put(1, "a").unwrap();
    let taken = put(2, "a").unwrap_err();
    let key = taken.unique_key().unwrap();
    assert_eq!(
        key,
        &UniqueKey {
            table: "t".into(),
            columns: vec!["s".into()]
        }
    );
    assert!(key.is(&"t", &["s"]) && !key.is(&"t", &["n"]) && !key.is(&"u", &["s"]));
}

#[test]
fn a_statement_binds_its_values_and_reads_back_by_column() {
    let (conn, _dir) = conn();
    let id: i64 = conn
        .row(
            Query::insert()
                .into_table("t")
                .columns(["n", "s", "b"])
                .values_panic([true.into(), "text".into(), vec![1u8, 2].into()])
                .returning_col("id"),
            |r| r.get(0),
        )
        .unwrap();
    conn.run(
        Query::insert()
            .into_table("t")
            .columns(["n", "s"])
            .values_panic([7u32.into(), None::<String>.into()]),
    )
    .unwrap();
    let rows: Vec<Read> = conn
        .rows(
            Query::select()
                .columns(["n", "n", "s", "b"])
                .from("t")
                .order_by("id", Order::Asc),
            |r| Ok::<_, Error>((r.get(0)?, r.get(1)?, r.get("s")?, r.get(3)?)),
        )
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(
        rows,
        [
            (1, true, Some("text".to_owned()), Some(vec![1, 2])),
            (7, true, None, None),
        ]
    );
}

#[test]
fn a_null_of_any_type_binds_where_the_column_fixes_its_type() {
    let (conn, _dir) = conn();
    conn.run(
        Query::insert()
            .into_table("t")
            .columns(["n", "s", "b"])
            .values_panic([
                None::<String>.into(),
                None::<i64>.into(),
                None::<bool>.into(),
            ]),
    )
    .unwrap();
    let nulls = conn
        .row(
            Query::select()
                .expr(Expr::col("n").is_null())
                .from("t")
                .and_where(Expr::col("s").is_null().and(Expr::col("b").is_null())),
            |r| r.get::<_, bool>(0),
        )
        .unwrap();
    assert!(nulls);
}

#[test]
fn a_sum_over_integers_reads_as_an_integer() {
    let (conn, _dir) = conn();
    insert_n(&conn, 2);
    insert_n(&conn, 3);
    let sum = conn
        .row(Query::select().expr(Expr::col("n").sum()).from("t"), |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn an_upsert_keeps_the_greater_value_and_no_row_is_none() {
    let (conn, _dir) = conn();
    let upsert = |n: i64| {
        conn.run(
            Query::insert()
                .into_table("t")
                .columns(["id", "n"])
                .values_panic([1.into(), n.into()])
                .on_conflict(
                    OnConflict::column("id")
                        .value(
                            "n",
                            Func::greatest([Expr::col(("t", "n")), Expr::col(("excluded", "n"))]),
                        )
                        .to_owned(),
                ),
        )
        .unwrap();
    };
    upsert(5);
    upsert(3);
    let read = |id: i64| {
        conn.row(
            Query::select()
                .column("n")
                .from("t")
                .and_where(Expr::col("id").eq(id)),
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .unwrap()
    };
    assert_eq!(read(1), Some(5));
    assert_eq!(read(2), None);
}

#[test]
fn a_value_out_of_the_integer_range_is_refused() {
    let (conn, _dir) = conn();
    let err = conn
        .run(
            Query::insert()
                .into_table("t")
                .columns(["n"])
                .values_panic([u64::MAX.into()]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Bind(_)), "{err:?}");
}

/// The migrations the database at `path` records, oldest first.
fn history(path: &std::path::Path) -> Vec<i64> {
    open_conn(path)
        .rows(
            Query::select()
                .column("version")
                .from("refinery_schema_history")
                .order_by("version", Order::Asc),
            |r| r.get(0),
        )
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_store_a_newer_build_migrated_opens_unchanged_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    let clock = || Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = open_store(&path, clock());
    assert_eq!(store.schema(), Outcome::Current);
    drop(store);
    let known = *history(&path).last().unwrap();
    let newer = known + 1;
    open_conn(&path)
        .run(
            Query::insert()
                .into_table("refinery_schema_history")
                .columns(["version", "name", "applied_on", "checksum"])
                .values_panic([
                    newer.into(),
                    "future".into(),
                    "2030-01-01T00:00:00Z".into(),
                    "1".into(),
                ]),
        )
        .unwrap();
    let store = open_store(&path, clock());
    assert_eq!(
        store.schema(),
        Outcome::Ahead {
            found: newer,
            known
        }
    );
    drop(store);
    assert_eq!(history(&path).last(), Some(&newer), "history untouched");
}
