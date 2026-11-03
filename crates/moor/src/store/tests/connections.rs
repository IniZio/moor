//! The store's connections: readers that only read, a connection the server dropped, and no
//! store call on a tokio runtime thread.

use moor_core::BoxId;

use super::{insert, new_box, open};
use crate::store::db::{Db, DbExt as _};
use crate::store::testing::{open_conn, postgres_only};

#[test]
fn a_reader_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let reader = store.reader().unwrap();
    let wrote = reader.execute("INSERT INTO meta (k, v) VALUES ('reader', 'wrote')", []);
    assert!(wrote.is_err(), "a reader wrote a row");
    drop(reader);
    let rows: i64 = store
        .reader()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM meta WHERE k = 'reader'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 0);
}

/// The server process behind `conn`.
fn backend_pid(conn: &dyn Db) -> i64 {
    conn.query_row("SELECT pg_backend_pid()", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_dropped_connection_fails_its_next_call_and_the_one_after_reconnects() {
    postgres_only!("no server drops a SQLite connection");
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "before");
    let writer = backend_pid(&*store.writer());
    let reader = backend_pid(&store.reader().unwrap());
    let admin = open_conn(&dir.path().join("state").join("moor.db"));
    for pid in [writer, reader] {
        let ended: bool = admin
            .query_row("SELECT pg_terminate_backend(?1, 5000)", [pid], |r| r.get(0))
            .unwrap();
        assert!(ended, "backend {pid} did not end");
    }

    assert!(
        store.apply(insert("during")).is_err(),
        "a write on a dropped connection"
    );
    new_box(&store, "after");
    assert!(store.box_row(id).is_err(), "a read on a dropped connection");
    assert!(store.box_row(id).unwrap().is_some());
    let names: Vec<String> = store
        .reader()
        .unwrap()
        .prepare("SELECT name FROM boxes ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(names, ["before", "after"]);
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
