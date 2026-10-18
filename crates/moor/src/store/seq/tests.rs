//! Tests for per-repo Box name allocation.

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::BoxName;
use rusqlite::TransactionBehavior;

use super::next_name;
use crate::store::tests::open;

/// The stored high-water mark for `repo`, if any.
fn mark(store: &crate::store::Store, repo: &str) -> Option<i64> {
    store
        .writer()
        .unwrap()
        .query_row("SELECT next FROM repo_seq WHERE repo = ?1", [repo], |r| {
            r.get(0)
        })
        .ok()
}

#[test]
fn unnamed_boxes_count_up_per_repo() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let names: Vec<String> = ["acme", "acme", "other"]
        .iter()
        .map(|repo| store.insert_unnamed(repo).unwrap().1.as_str().to_owned())
        .collect();
    assert_eq!(names, ["acme/1", "acme/2", "other/1"]);
}

#[test]
fn numbers_are_not_reused_after_close() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let (first, name) = store.insert_unnamed("acme").unwrap();
    assert_eq!(name.as_str(), "acme/1");
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE boxes SET state = 'closed' WHERE id = ?1",
            [first.get()],
        )
        .unwrap();
    let (_, next) = store.insert_unnamed("acme").unwrap();
    assert_eq!(next.as_str(), "acme/2");
    store
        .writer()
        .unwrap()
        .execute("UPDATE boxes SET state = 'closed'", [])
        .unwrap();
    let (_, third) = store.insert_unnamed("acme").unwrap();
    assert_eq!(third.as_str(), "acme/3");
}

#[test]
fn hand_picked_name_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    store
        .apply(Decision::insert(NewTask {
            name: BoxName::parse("acme/1").unwrap(),
            repo: "acme".into(),
        }))
        .unwrap();
    let (_, name) = store.insert_unnamed("acme").unwrap();
    assert_eq!(name.as_str(), "acme/2");
    assert_eq!(mark(&store, "acme"), Some(2));
}

#[test]
fn allocation_rolls_back_with_its_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    store.insert_unnamed("acme").unwrap();
    {
        let mut conn = store.writer().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(next_name(&tx, "acme").unwrap().as_str(), "acme/2");
        drop(tx);
        drop(conn);
    }
    assert_eq!(mark(&store, "acme"), Some(1));
    let (_, name) = store.insert_unnamed("acme").unwrap();
    assert_eq!(name.as_str(), "acme/2");
}
