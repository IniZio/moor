//! Tests for opening and listing Sessions of a Box.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::{BoxId, ProviderId};

use crate::store::tests::{new_task, open};
use crate::store::StoreError;

#[test]
fn new_box_has_one_starting_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    let sessions = store.box_sessions(id).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].ordinal, 1);
    assert_eq!(sessions[0].state, SessionState::Starting);
}

#[test]
fn open_session_numbers_after_the_last() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    let second = store.open_session(id).unwrap();
    let third = store.open_session(id).unwrap();
    assert_eq!((second.ordinal, third.ordinal), (2, 3));
    assert_eq!(second.state, SessionState::Starting);
    let listed = store.box_sessions(id).unwrap();
    let ordinals: Vec<i64> = listed.iter().map(|s| s.ordinal).collect();
    assert_eq!(ordinals, [1, 2, 3]);
    assert_eq!(listed[1], second);
    assert_eq!(listed[2], third);
}

#[test]
fn sessions_stay_with_their_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = new_task(&store, "acme/1");
    let b = new_task(&store, "acme/2");
    store.open_session(a).unwrap();
    assert_eq!(store.box_sessions(a).unwrap().len(), 2);
    assert_eq!(store.box_sessions(b).unwrap().len(), 1);
    assert_eq!(store.open_session(b).unwrap().ordinal, 2);
}

#[test]
fn open_session_on_unknown_box_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let err = store.open_session(BoxId::new(999)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");
}

fn session_states(store: &crate::store::Store, id: BoxId) -> Vec<String> {
    store
        .writer()
        .unwrap()
        .prepare("SELECT state FROM sessions WHERE box = ? ORDER BY ordinal")
        .unwrap()
        .query_map([id.get()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn add_working_session(store: &crate::store::Store, id: BoxId) {
    store
        .writer()
        .unwrap()
        .execute(
            "INSERT INTO sessions (box, ordinal, state, started_at) VALUES (?, 2, 'working', 0)",
            [id.get()],
        )
        .unwrap();
}

#[test]
fn closing_a_box_ends_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    let admit = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admit).unwrap();
    let up = TaskInput::BoxReady {
        setup: SetupOutcome::Ok,
    };
    store.apply(Decision::new(id).task_input(up)).unwrap();
    add_working_session(&store, id);
    store
        .apply(Decision::new(id).task_input(TaskInput::Close))
        .unwrap();
    assert_eq!(session_states(&store, id), ["ended", "ended"]);
}

#[test]
fn rejecting_a_box_ends_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    add_working_session(&store, id);
    let reject = TaskInput::Rejected {
        reason: "no".into(),
    };
    store.apply(Decision::new(id).task_input(reject)).unwrap();
    assert_eq!(session_states(&store, id), ["ended", "ended"]);
}
