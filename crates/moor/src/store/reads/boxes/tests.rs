//! Tests for the open-Box lookups by repo and by checkout path.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};

use crate::testing::clock::FakeClock;

use super::Store;

/// Open a store in a temp dir.
fn open(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a Box named `name` in `repo`.
fn add(store: &Store, name: &str, repo: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: repo.into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// Insert a Box whose worktree is `path`, bound or not.
fn add_at(store: &Store, name: &str, repo: &str, path: &str, bound: bool) -> BoxId {
    let id = add(store, name, repo);
    let fields = Fields {
        worktree: Field::Set(path.into()),
        bound: Field::Set(bound),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(fields)).unwrap();
    id
}

/// Close a Box.
fn close(store: &Store, id: BoxId) {
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
}

/// Ids of the rows, in order.
fn ids(rows: &[crate::store::TaskRow]) -> Vec<BoxId> {
    rows.iter().map(|r| r.id).collect()
}

#[test]
fn by_repo_returns_open_boxes_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = add(&store, "a", "repo-match");
    let _other = add(&store, "o", "repo-other");
    let b = add(&store, "b", "repo-match");
    let rows = store.open_boxes_by_repo("repo-match").unwrap();
    assert_eq!(ids(&rows), vec![a, b]);
}

#[test]
fn by_repo_without_match_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    add(&store, "a", "repo-present");
    assert!(store.open_boxes_by_repo("repo-absent").unwrap().is_empty());
}

#[test]
fn by_repo_excludes_closed_boxes() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let gone = add(&store, "a", "repo-closed");
    let kept = add(&store, "b", "repo-closed");
    close(&store, gone);
    let rows = store.open_boxes_by_repo("repo-closed").unwrap();
    assert_eq!(ids(&rows), vec![kept]);
}

#[test]
fn by_path_unbound_matches_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add_at(&store, "a", "repo-u1", "/w/one", false);
    add_at(&store, "b", "repo-u1", "/w/two", false);
    let row = store.open_box_by_path("/w/one").unwrap().unwrap();
    assert_eq!(row.id, id);
}

#[test]
fn by_path_unbound_without_match_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    add_at(&store, "a", "repo-u2", "/w/one", false);
    assert!(store.open_box_by_path("/w/missing").unwrap().is_none());
}

#[test]
fn by_path_unbound_excludes_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add_at(&store, "a", "repo-u3", "/w/one", false);
    close(&store, id);
    assert!(store.open_box_by_path("/w/one").unwrap().is_none());
}

#[test]
fn by_path_bound_matches_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add_at(&store, "a", "repo-b1", "/w/own", true);
    let row = store.open_box_by_path("/w/own").unwrap().unwrap();
    assert_eq!(row.id, id);
}

#[test]
fn by_path_bound_excludes_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let id = add_at(&store, "a", "repo-b3", "/w/own", true);
    close(&store, id);
    assert!(store.open_box_by_path("/w/own").unwrap().is_none());
}
