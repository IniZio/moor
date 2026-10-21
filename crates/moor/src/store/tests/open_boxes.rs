use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;

use super::{new_box, open};

#[test]
fn open_boxes_excludes_closed_and_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let live = new_box(&store, "a");
    let closed = new_box(&store, "b");
    let rejected = new_box(&store, "c");
    store
        .apply(
            Decision::new(closed)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    store
        .apply(
            Decision::new(rejected).lifecycle(BoxLifecycleInput::Rejected {
                reason: "no".into(),
            }),
        )
        .unwrap();
    let open_ids: Vec<_> = store
        .open_boxes()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(open_ids, vec![live]);
}

#[test]
fn a_pinned_name_means_only_its_pinned_box_after_another_of_that_name_opens() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let first = new_box(&store, "a");
    let close = Decision::new(first)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    store.apply(close).unwrap();
    let name = crate::testing::box_name("a");
    let db = dir.path().join("state").join("moor.db");
    let pinned = super::super::Store::open(&db, clock)
        .unwrap()
        .pinned([(name.clone(), first)].into());
    let second = new_box(&store, "a");
    assert_eq!(
        store.latest_box_by_name(&name).unwrap().map(|r| r.id),
        Some(second)
    );
    assert_eq!(
        pinned.latest_box_by_name(&name).unwrap().map(|r| r.id),
        Some(first)
    );
    assert!(
        pinned.open_box_by_name(&name).unwrap().is_none(),
        "the pinned Box is closed"
    );
}
