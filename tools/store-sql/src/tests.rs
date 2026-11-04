use std::fs;

use super::*;

fn lines(src: &str) -> Vec<usize> {
    scan(src, true)
        .unwrap()
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

#[test]
fn a_text_sql_call_is_flagged_whatever_its_argument() {
    assert_eq!(lines("fn f() { conn.execute(\"x\", []); }"), [1]);
    assert_eq!(
        lines("fn f() { conn.query_row(&format!(\"{A} x\"), [], g); }"),
        [1]
    );
    assert_eq!(lines("fn f() { let s = tx.prepare(SQL)?; }"), [1]);
}

#[test]
fn a_string_of_sql_is_flagged_inside_a_macro_too() {
    assert_eq!(lines("const A: &str = \"SELECT id FROM boxes\";"), [1]);
    assert_eq!(
        lines("fn f() {\n let s = format!(\"\\n  UPDATE {t} SET x = 1\"); }"),
        [2]
    );
    assert_eq!(lines("const N: &str = r\"ORDER BY id\";"), [1]);
}

#[test]
fn asking_for_or_rendering_one_backend_is_flagged() {
    assert_eq!(lines("fn f() { match conn.backend() { _ => {} } }"), [1]);
    assert_eq!(lines("use crate::store::db::Backend;"), [1]);
    assert_eq!(lines("fn f() { q.to_string(SqliteQueryBuilder); }"), [1]);
    assert_eq!(
        lines("fn f() { q.and_where(Expr::cust(\"json(x)\")); }"),
        [1]
    );
}

#[test]
fn sea_query_statements_prose_and_doc_comments_pass() {
    let src = "/// SELECT is what a reader runs.\n\
        fn f() { conn.run(Query::update().table(Boxes::Table).value(Boxes::Node, n)); \
        let e = \"Selected boxes\"; let w = \"WHEREVER\"; let b = self.backend; }";
    assert!(scan(src, true).unwrap().is_empty(), "{:?}", scan(src, true));
}

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, src) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, src).unwrap();
    }
    dir
}

const BAD: &str = "fn f() { conn.execute(\"DELETE FROM t\", []); }";

#[test]
fn naming_the_postgres_driver_is_flagged() {
    assert_eq!(lines("fn f(c: &mut postgres::Client) {}"), [1]);
    assert_eq!(lines("use tokio_postgres::NoTls;"), [1]);
}

#[test]
fn naming_the_sqlite_driver_is_flagged() {
    assert_eq!(
        lines("fn f() { let c = rusqlite::Connection::open(p); }"),
        [1, 1]
    );
    assert_eq!(lines("use crate::store::db::TransactionBehavior;"), [1]);
    assert_eq!(
        lines("fn f(e: E) { if let E::SqliteFailure(f, _) = e { f.code == ErrorCode::X; } }"),
        [1, 1]
    );
}

#[test]
fn outside_the_store_only_a_text_method_name_passes() {
    let src = "fn f() { self.execute(connect, &plan); s.prepare(); }";
    assert!(scan(src, false).unwrap().is_empty());
    assert_eq!(lines(src), [1, 1]);
    let raw = "fn f() { db.execute(\"UPDATE boxes SET a = 1\", []); }";
    assert_eq!(
        scan(raw, false).unwrap().len(),
        1,
        "the SQL string still is"
    );
}

#[test]
fn a_lone_sql_word_is_not_sql() {
    assert!(lines("const M: [&str; 2] = [\"DELETE\", \"UPDATE \"];").is_empty());
}

#[test]
fn the_seam_is_skipped_and_the_rest_of_the_crate_is_checked() {
    let dir = repo(&[
        ("crates/moor/src/store/db.rs", BAD),
        ("crates/moor/src/store/db/sqlite.rs", BAD),
        ("crates/moor/src/store/db/sqlite/migrate/tests.rs", BAD),
        ("crates/moor/src/store/runs.rs", BAD),
        ("crates/moor/src/store/runs/tests.rs", "fn ok() {}"),
        ("crates/moor/src/ops/start/tests.rs", BAD),
        (
            "crates/moor/src/ops/start.rs",
            "fn f() { self.execute(a, b); }",
        ),
    ]);
    let (checked, bad) = check(dir.path()).unwrap();
    assert_eq!(checked, 4);
    assert_eq!(
        bad.iter()
            .map(|b| b.split(": ").nth(1).unwrap())
            .collect::<Vec<_>>(),
        [
            "crates/moor/src/ops/start/tests.rs:1",
            "crates/moor/src/store/runs.rs:1",
            "crates/moor/src/store/runs.rs:1"
        ],
        "{bad:?}"
    );
}
