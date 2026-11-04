//! Events shown for a Box, including rows written by older versions.

use crate::store::db::DbExt as _;
use moor_schema::tables::Events;
use sea_query::Query;
use std::sync::Arc;

use super::super::run;
use super::{add, cx, req};
use crate::testing::driver::FakeDriver;

#[test]
fn old_closed_row_with_working_from_still_shows() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    let raw = crate::store::db::Conn::open_file(&cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.run(
        Query::insert()
            .into_table(Events::Table)
            .columns([Events::Box, Events::Kind, Events::Payload, Events::Ts])
            .values_panic([
                id.get().into(),
                "closed".into(),
                r#"{"from":"working"}"#.into(),
                0.into(),
            ]),
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
