//! Events shown for a Box, including rows written by older versions.

use std::sync::Arc;

use super::super::run;
use super::{add, cx, req};
use crate::testing::driver::FakeDriver;

#[test]
fn old_closed_row_with_working_from_still_shows() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    let raw = rusqlite::Connection::open(cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO events (box, kind, payload, ts) VALUES (?1, 'closed', '{\"from\":\"working\"}', 0)",
        [id.get()],
    )
    .unwrap();
    drop(raw);
    let v = run(&cx, &req(Some("a"), Some(100))).unwrap();
    let closed: Vec<_> = v
        .events
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "closed")
        .collect();
    let origins: Vec<_> = closed.iter().map(|e| e.payload.get("from")).collect();
    assert_eq!(origins, [Some(&serde_json::json!("working"))]);
}
