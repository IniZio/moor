use rusqlite::types::Type;
use rusqlite::Connection;

use super::{params, Conn, DbExt as _, Error, OptionalExtension as _, TransactionBehavior};

type Read = (i64, bool, Option<String>, Option<Vec<u8>>);

fn conn() -> Conn {
    let conn = Conn::sqlite(Connection::open_in_memory().unwrap());
    conn.execute(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, s TEXT, b BLOB)",
        (),
    )
    .unwrap();
    conn
}

#[test]
fn values_bind_and_read_back_as_rusqlite_converts_them() {
    let conn = conn();
    conn.execute(
        "INSERT INTO t (n, s, b) VALUES (?, ?, ?)",
        params![true, "text", vec![1u8, 2]],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO t (n, s, b) VALUES (?1, ?2, ?3)",
        (7u32, None::<String>, None::<Vec<u8>>),
    )
    .unwrap();
    let rows: Vec<Read> = conn
        .prepare("SELECT n, n, s, b FROM t ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            (1, true, Some("text".to_owned()), Some(vec![1, 2])),
            (7, true, None, None),
        ]
    );
}

#[test]
fn a_column_of_the_wrong_type_is_refused_with_its_name() {
    let conn = conn();
    conn.execute("INSERT INTO t (s) VALUES ('x')", []).unwrap();
    let err = conn
        .query_row("SELECT s FROM t", [], |r| r.get::<_, i64>(0))
        .unwrap_err();
    assert!(
        matches!(&err, Error::Sqlite(rusqlite::Error::InvalidColumnType(0, name, Type::Text)) if name == "s"),
        "{err:?}"
    );
    let err = conn
        .query_row("SELECT -1 AS n", [], |r| r.get::<_, u32>("n"))
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Sqlite(rusqlite::Error::IntegralValueOutOfRange(0, -1))
        ),
        "{err:?}"
    );
}

#[test]
fn no_row_is_an_error_that_optional_turns_into_none() {
    let conn = conn();
    let none = conn
        .query_row("SELECT n FROM t", [], |r| r.get::<_, i64>(0))
        .optional()
        .unwrap();
    assert_eq!(none, None);
}

#[test]
fn rows_are_mapped_only_as_they_are_taken() {
    let conn = conn();
    conn.execute("INSERT INTO t (n) VALUES (1), (2)", [])
        .unwrap();
    let mut stmt = conn.prepare("SELECT n FROM t ORDER BY n").unwrap();
    let mut rows = stmt
        .query_and_then([], |r| match r.get::<_, i64>(0)? {
            1 => Ok(1),
            n => Err(rusqlite::Error::IntegralValueOutOfRange(0, n)),
        })
        .unwrap();
    assert_eq!(rows.next().transpose().unwrap(), Some(1));
}

#[test]
fn a_dropped_transaction_rolls_back_and_a_committed_one_stays() {
    let mut conn = conn();
    {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute("INSERT INTO t (n) VALUES (1)", []).unwrap();
    }
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO t (n) VALUES (2)", []).unwrap();
    tx.commit().unwrap();
    let held: Vec<i64> = conn
        .prepare("SELECT n FROM t")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(held, [2]);
}

/// A store on the backend store tests run on, in a temp dir.
fn store() -> (tempfile::TempDir, crate::store::Store) {
    let dir = tempfile::tempdir().unwrap();
    let clock = std::sync::Arc::new(crate::testing::clock::FakeClock::new(
        moor_core::seam::clock::Ts::from_millis(0),
    ));
    let store = crate::store::testing::open_store(&dir.path().join("moor.db"), clock);
    (dir, store)
}

type Typed = (i64, i32, bool, String, Vec<u8>, f64, i64);
type Nulls = (
    Option<i64>,
    Option<i32>,
    Option<bool>,
    Option<String>,
    Option<Vec<u8>>,
    Option<f64>,
    Option<i64>,
);

#[test]
fn each_column_type_reads_back_as_it_was_bound() {
    let (_dir, store) = store();
    let db = store.writer();
    db.execute(
        "CREATE TABLE seam (i8 BIGINT, i4 INTEGER, b BOOLEAN, t TEXT, y BYTEA, \
         f DOUBLE PRECISION, n NUMERIC)",
        (),
    )
    .unwrap();
    db.execute(
        "INSERT INTO seam VALUES (?, ?, ?, ?, ?, ?, 12345678901)",
        (i64::MAX, -7i32, true, "text", vec![0u8, 255], 1.5f64),
    )
    .unwrap();
    let read: Typed = db
        .query_row("SELECT i8, i4, b, t, y, f, n FROM seam", [], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .unwrap();
    drop(db);
    assert_eq!(
        read,
        (
            i64::MAX,
            -7,
            true,
            "text".to_owned(),
            vec![0, 255],
            1.5,
            12_345_678_901
        )
    );
}

#[test]
fn a_null_of_each_column_type_reads_back_as_none() {
    let (_dir, store) = store();
    let db = store.writer();
    db.execute(
        "CREATE TABLE seam (i8 BIGINT, i4 INTEGER, b BOOLEAN, t TEXT, y BYTEA, \
         f DOUBLE PRECISION, n NUMERIC)",
        (),
    )
    .unwrap();
    db.execute(
        "INSERT INTO seam VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        (
            None::<i64>,
            None::<i32>,
            None::<bool>,
            None::<String>,
            None::<Vec<u8>>,
            None::<f64>,
        ),
    )
    .unwrap();
    let read: Nulls = db
        .query_row("SELECT i8, i4, b, t, y, f, n FROM seam", [], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .unwrap();
    drop(db);
    assert_eq!(read, (None, None, None, None, None, None, None));
}

#[test]
fn a_sum_of_integers_reads_as_an_integer() {
    let (_dir, store) = store();
    let db = store.writer();
    db.execute("CREATE TABLE seam (n BIGINT)", ()).unwrap();
    db.execute("INSERT INTO seam VALUES (?), (?)", (5, 6))
        .unwrap();
    let sum: i64 = db
        .query_row("SELECT SUM(n) FROM seam", [], |r| r.get(0))
        .unwrap();
    drop(db);
    assert_eq!(sum, 11);
}

#[test]
fn a_duplicate_key_is_a_unique_violation_of_its_index() {
    let (_dir, store) = store();
    let db = store.writer();
    db.execute("CREATE TABLE seam (k TEXT)", ()).unwrap();
    db.execute("CREATE UNIQUE INDEX seam_k ON seam (k)", ())
        .unwrap();
    db.execute("INSERT INTO seam VALUES (?)", ["a"]).unwrap();
    let err = db
        .execute("INSERT INTO seam VALUES (?)", ["a"])
        .unwrap_err();
    drop(db);
    assert!(
        err.is_unique_violation(|m| m.contains("seam.k"), "seam_k"),
        "{err:?}"
    );
    assert!(!err.is_unique_violation(|m| m.contains("other.k"), "other_k"));
}
