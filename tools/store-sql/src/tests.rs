use std::fs;

use super::*;

fn lines(src: &str) -> Vec<usize> {
    scan(src).unwrap().into_iter().map(|(l, _)| l).collect()
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
    assert!(scan(src).unwrap().is_empty(), "{:?}", scan(src));
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
fn the_seam_is_skipped_and_the_rest_of_the_store_is_checked() {
    let dir = repo(&[
        ("crates/moor/src/store/db.rs", BAD),
        ("crates/moor/src/store/db/sqlite.rs", BAD),
        ("crates/moor/src/store/runs.rs", BAD),
        ("crates/moor/src/store/runs/tests.rs", "fn ok() {}"),
    ]);
    let (checked, bad) = check(dir.path(), "").unwrap();
    assert_eq!(checked, 2);
    assert_eq!(bad.len(), 2, "{bad:?}");
    assert!(bad
        .iter()
        .all(|b| b.starts_with("store-sql: crates/moor/src/store/runs.rs:1")));
}

#[test]
fn a_listed_file_is_skipped_until_it_is_clean() {
    let listed = "crates/moor/src/store/runs.rs # not converted yet\n";
    let dir = repo(&[("crates/moor/src/store/runs.rs", BAD)]);
    assert_eq!(check(dir.path(), listed).unwrap(), (0, Vec::new()));
    let clean = repo(&[("crates/moor/src/store/runs.rs", "fn ok() {}")]);
    assert!(check(clean.path(), listed)
        .unwrap_err()
        .contains("is clean"));
    let gone = repo(&[("crates/moor/src/store/other.rs", "fn ok() {}")]);
    assert!(check(gone.path(), listed)
        .unwrap_err()
        .contains("not a store file"));
}
