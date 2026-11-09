use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxHandle, BoxId};

use super::{new_box, open, provider};
use crate::store::Store;

/// A box with a Ready box, as after provisioning.
fn ready_box(store: &Store) -> BoxId {
    let id = new_box(store, "t");
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Admitted)
                .hands_input(HandsInc::FIRST, HandsInput::Requested)
                .hands(HandsInc::FIRST, provider()),
        )
        .unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: SetupOutcome::Ok,
                })
                .session(SessionInput::Started)
                .hands_input(
                    HandsInc::FIRST,
                    HandsInput::Acquired {
                        handle: BoxHandle::new("h1"),
                    },
                ),
        )
        .unwrap();
    id
}

#[test]
fn fresh_box_has_no_sleep_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = ready_box(&store);
    assert_eq!(
        store
            .box_row(id)
            .unwrap()
            .unwrap()
            .hands
            .unwrap()
            .sleep_reason,
        None
    );
}

#[test]
fn sleep_reason_is_set_without_changing_states() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = ready_box(&store);
    let before = store.box_row(id).unwrap().unwrap();
    store
        .apply(Decision::new(id).hands(
            HandsInc::FIRST,
            HandsFields {
                sleep_reason: Field::Set("no wake".into()),
                ..HandsFields::default()
            },
        ))
        .unwrap();
    let after = store.box_row(id).unwrap().unwrap();
    let b = after.hands.unwrap();
    assert_eq!(b.sleep_reason.as_deref(), Some("no wake"));
    assert_eq!(
        (after.lifecycle, b.state),
        (before.lifecycle, before.hands.unwrap().state)
    );
}

#[test]
fn sleep_reason_clears_in_the_same_apply_as_a_box_input() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = ready_box(&store);
    let reason = |f| {
        Decision::new(id).hands(
            HandsInc::FIRST,
            HandsFields {
                sleep_reason: f,
                ..HandsFields::default()
            },
        )
    };
    store.apply(reason(Field::Set("no wake".into()))).unwrap();
    store
        .apply(reason(Field::Clear).hands_input(HandsInc::FIRST, HandsInput::Sleep))
        .unwrap();
    let b = store.box_row(id).unwrap().unwrap().hands.unwrap();
    assert_eq!((b.state, b.sleep_reason), (HandsState::Asleep, None));
}
