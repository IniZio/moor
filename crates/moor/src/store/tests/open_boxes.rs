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
