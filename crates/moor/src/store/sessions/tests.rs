//! Tests for opening and listing Sessions of a Box.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::{BoxId, ProviderId};

use crate::store::tests::{new_task, open};
use crate::store::StoreError;
use moor_core::seam::clock::Clock;

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

fn ready_box(store: &crate::store::Store) -> BoxId {
    let id = new_task(store, "acme/1");
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
    id
}

fn ended_events(store: &crate::store::Store, id: BoxId) -> Vec<String> {
    store
        .writer()
        .unwrap()
        .prepare("SELECT payload FROM events WHERE box = ? AND kind = 'session_ended'")
        .unwrap()
        .query_map([id.get()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn end_current_session_ends_session_launch_and_records_event() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = ready_box(&store);
    let launch = store.begin_session(id, "ref", None).unwrap();
    clock.sleep(std::time::Duration::from_secs(5));
    assert!(store.end_current_session(id, "exited", Some(3)).unwrap());
    assert_eq!(session_states(&store, id), ["ended"]);
    let conn = store.writer().unwrap();
    let (ended_at, exit): (Option<i64>, Option<i32>) = conn
        .query_row(
            "SELECT ended_at, exit FROM launches WHERE id = ?",
            [launch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((ended_at, exit), (Some(1_005_000), Some(3)));
    let session_end: Option<i64> = conn
        .query_row(
            "SELECT ended_at FROM sessions WHERE box = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(session_end, Some(1_005_000));
    drop(conn);
    let events = ended_events(&store, id);
    assert_eq!(events.len(), 1);
    let payload: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
    let want = serde_json::json!({"session": 1, "cause": "exited", "exit": 3});
    assert_eq!(payload, want);
}

#[test]
fn end_current_session_twice_changes_nothing_the_second_time() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = ready_box(&store);
    assert!(store.end_current_session(id, "exited", None).unwrap());
    let before = store.task(id).unwrap().unwrap();
    clock.sleep(std::time::Duration::from_secs(5));
    assert!(!store.end_current_session(id, "again", Some(1)).unwrap());
    assert_eq!(ended_events(&store, id).len(), 1);
    assert_eq!(
        store.task(id).unwrap().unwrap().updated_at,
        before.updated_at
    );
}

#[test]
fn end_current_session_on_unknown_box_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let err = store
        .end_current_session(BoxId::new(999), "exited", None)
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");
}

#[test]
fn end_current_session_ends_only_the_latest_ordinal() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "acme/1");
    add_working_session(&store, id);
    assert!(store.end_current_session(id, "requested", None).unwrap());
    assert_eq!(session_states(&store, id), ["starting", "ended"]);
}

#[test]
fn ready_box_with_ended_session_reads_idle() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = ready_box(&store);
    clock.sleep(std::time::Duration::from_secs(5));
    store.end_current_session(id, "exited", None).unwrap();
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(row.state.as_str(), "idle");
    assert_eq!(row.idle_since.unwrap().as_millis(), 1_005_000);
}
