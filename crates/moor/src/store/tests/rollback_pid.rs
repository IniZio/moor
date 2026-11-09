//! An intent row still carries a pid, for the previous binary a failed deploy rolls back to.

use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::BoxId;
use moor_schema::tables::BoxState;
use sea_query::{Expr, ExprTrait as _, Query};

use super::{new_box, open};
use crate::store::db::DbExt as _;
use crate::store::Store;

/// The pid column of Box `id`'s intent, as the previous binary reads it.
fn recorded_pid(store: &Store, id: BoxId) -> Option<i64> {
    let q = Query::select()
        .column(BoxState::IntentPid)
        .from(BoxState::Table)
        .and_where(Expr::col(BoxState::Box).eq(id.get()))
        .take();
    store.reader().unwrap().row(q, |r| r.get(0)).unwrap()
}

#[test]
fn an_intent_written_now_carries_this_process_pid_and_a_cleared_one_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, _clock) = open(tmp.path());
    let id = new_box(&store, "b");
    let set = Decision::new(id).intent(IntentChange::Set(Intent::Setup));
    store.apply(set).unwrap();
    assert_eq!(
        recorded_pid(&store, id),
        Some(i64::from(std::process::id()))
    );
    store
        .apply(Decision::new(id).intent(IntentChange::Clear))
        .unwrap();
    assert_eq!(recorded_pid(&store, id), None);
}
