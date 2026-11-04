use rusqlite::Connection;
use sea_query::{Expr, ExprTrait as _, Func, OnConflict, Order, Query};

use super::{
    unique_violation, Conn, Db, DbExt as _, Error, OptionalExtension as _, Type, UniqueKey,
};

type Read = (i64, bool, Option<String>, Option<Vec<u8>>);

fn conn() -> Conn {
    let conn = Conn::sqlite(Connection::open_in_memory().unwrap());
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, s TEXT, b BLOB); \
         CREATE UNIQUE INDEX t_s ON t (s);",
    )
    .unwrap();
    conn
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
    let conn = conn();
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
        matches!(&err, Error::InvalidColumnType(0, name, Type::Text) if name == "s"),
        "{err:?}"
    );
    let err = conn
        .row(Query::select().expr_as(Expr::val(-1), "n"), |r| {
            r.get::<_, u32>("n")
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::IntegralValueOutOfRange(0, -1)),
        "{err:?}"
    );
}

#[test]
fn no_row_is_an_error_that_optional_turns_into_none() {
    let conn = conn();
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
    let conn = conn();
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
                n => Err(Error::IntegralValueOutOfRange(0, n)),
            },
        )
        .unwrap();
    assert_eq!(rows.next().transpose().unwrap(), Some(1));
}

#[test]
fn a_dropped_transaction_rolls_back_and_a_committed_one_stays() {
    let mut conn = conn();
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
    let conn = conn();
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
    let key = unique_violation(&taken).unwrap();
    assert_eq!(
        key,
        UniqueKey {
            table: "t".into(),
            columns: vec!["s".into()]
        }
    );
    assert!(key.is(&"t", &["s"]) && !key.is(&"t", &["n"]) && !key.is(&"u", &["s"]));
    assert_eq!(
        unique_violation(&put(1, "b").unwrap_err()),
        None,
        "a primary key"
    );
}

#[test]
fn a_statement_binds_its_values_and_reads_back_by_column() {
    let conn = conn();
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
fn an_upsert_keeps_the_greater_value_and_no_row_is_none() {
    let conn = conn();
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
                            Func::greatest([Expr::col("n"), Expr::col(("excluded", "n"))]),
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
    let conn = conn();
    let err = conn
        .run(
            Query::insert()
                .into_table("t")
                .columns(["n"])
                .values_panic([u64::MAX.into()]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::ToSqlConversionFailure(_)), "{err:?}");
}
