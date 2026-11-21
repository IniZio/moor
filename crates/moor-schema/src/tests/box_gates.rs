//! `V97__box_gates` leaves every Box as it was, gives it no gates, keeps one line per number for
//! each Box's gates, and drops a Box's lines with the Box.

use moor_migrate::Backend;
use rusqlite::Connection;

/// Two Boxes, as the store held them before the step.
const BEFORE: &str = "INSERT INTO boxes (name, created_at) VALUES ('a', 1), ('b', 2);";

/// Each Box's name and gates, `-` for none.
const BOXES: &str = "SELECT name || '|' || COALESCE(gates, '-') FROM boxes ORDER BY id";

/// Each line as `box|n`.
const LINES: &str = "SELECT CAST(box AS TEXT) || '|' || CAST(n AS TEXT) FROM box_gate_lines \
    ORDER BY box, n";

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let got = stmt.query_map([], |r| r.get(0)).unwrap();
    got.collect::<Result<_, _>>().unwrap()
}

fn line(box_id: i64, n: i64) -> String {
    format!("INSERT INTO box_gate_lines (box, n, at, entry) VALUES ({box_id}, {n}, 1, '{{}}')")
}

#[test]
fn sqlite_keeps_the_boxes_refuses_a_line_number_twice_and_cascades_the_lines() {
    let (before, step) = super::before_and_step("V97__box_gates", Backend::Sqlite);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(&before).unwrap();
    conn.execute_batch(BEFORE).unwrap();
    conn.execute_batch(&format!("BEGIN; {step} COMMIT;"))
        .unwrap();
    assert_eq!(rows(&conn, BOXES), ["a|-", "b|-"]);
    conn.execute_batch(&format!("{}; {}; {};", line(1, 1), line(1, 2), line(2, 1)))
        .unwrap();
    assert!(conn.execute_batch(&line(1, 2)).is_err());
    assert!(conn.execute_batch(&line(3, 1)).is_err());
    assert_eq!(rows(&conn, LINES), ["1|1", "1|2", "2|1"]);
    conn.execute_batch("DELETE FROM boxes WHERE id = 1")
        .unwrap();
    assert_eq!(rows(&conn, LINES), ["2|1"]);
}
