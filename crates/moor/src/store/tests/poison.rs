//! A panic while a thread holds the writer must not break later writes.

use std::sync::Arc;

use moor_schema::tables::Boxes;
use sea_query::Query;

use super::{new_box, open};
use crate::store::db::DbExt as _;

#[test]
#[expect(
    clippy::significant_drop_tightening,
    reason = "the thread must panic while it still holds the writer"
)]
fn a_panic_holding_the_writer_does_not_break_later_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let store = Arc::new(store);
    let held = Arc::clone(&store);
    let joined = std::thread::spawn(move || {
        let mut conn = held.writer();
        let tx = conn.transaction().unwrap();
        tx.run(
            Query::insert()
                .into_table(Boxes::Table)
                .columns([
                    Boxes::Name,
                    Boxes::Repo,
                    Boxes::State,
                    Boxes::CreatedAt,
                    Boxes::UpdatedAt,
                ])
                .values_panic([
                    "ghost".into(),
                    "o/r".into(),
                    "checking".into(),
                    0_i64.into(),
                    0_i64.into(),
                ]),
        )
        .unwrap();
        std::panic::resume_unwind(Box::new("panic inside the writer's transaction"));
    })
    .join();
    assert!(joined.is_err());
    let id = new_box(&store, "after");
    let open = store.open_boxes().unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open.first().map(|b| b.id), Some(id), "{open:?}");
}
