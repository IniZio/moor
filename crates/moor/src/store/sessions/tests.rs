//! Tests for opening and listing Sessions of a Box.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::{BoxId, ProviderId};

use crate::store::tests::{new_box, open};
use crate::store::StoreError;
use moor_core::seam::clock::Clock;

fn set_current(store: &crate::store::Store, id: BoxId, state: &str) {
    store
        .writer()
        .execute(
            "UPDATE sessions SET state = ? WHERE box = ? AND ordinal = (SELECT MAX(ordinal) FROM sessions WHERE box = ?)",
            (state, id.get(), id.get()),
        )
        .unwrap();
}

fn end_current(store: &crate::store::Store, id: BoxId) {
    set_current(store, id, "ended");
}

#[test]
fn open_session_is_refused_while_the_current_session_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    for state in ["starting", "working", "idle"] {
        set_current(&store, id, state);
        let err = store.open_session(id).unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "{state}: {err:?}");
    }
    assert_eq!(store.box_sessions(id).unwrap().len(), 1);
}

#[test]
fn open_session_is_allowed_after_the_current_session_ended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    end_current(&store, id);
    assert_eq!(store.open_session(id).unwrap().ordinal, 2);
}

#[test]
fn open_session_is_allowed_when_the_box_has_no_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    let conn = store.writer();
    conn.execute("UPDATE events SET session = NULL WHERE box = ?", [id.get()])
        .unwrap();
    conn.execute("DELETE FROM sessions WHERE box = ?", [id.get()])
        .unwrap();
    drop(conn);
    assert_eq!(store.open_session(id).unwrap().ordinal, 1);
}

#[test]
fn new_box_has_one_starting_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    let sessions = store.box_sessions(id).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].ordinal, 1);
    assert_eq!(sessions[0].state, SessionState::Starting);
}

#[test]
fn open_session_numbers_after_the_last() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    end_current(&store, id);
    let second = store.open_session(id).unwrap();
    end_current(&store, id);
    let third = store.open_session(id).unwrap();
    assert_eq!((second.ordinal, third.ordinal), (2, 3));
    assert_eq!(second.state, SessionState::Starting);
    let listed = store.box_sessions(id).unwrap();
    let ordinals: Vec<i64> = listed.iter().map(|s| s.ordinal).collect();
    assert_eq!(ordinals, [1, 2, 3]);
    assert_eq!(listed[1].id, second.id);
    assert_eq!(listed[2], third);
}

#[test]
fn sessions_stay_with_their_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = new_box(&store, "acme/1");
    let b = new_box(&store, "acme/2");
    end_current(&store, a);
    store.open_session(a).unwrap();
    assert_eq!(store.box_sessions(a).unwrap().len(), 2);
    assert_eq!(store.box_sessions(b).unwrap().len(), 1);
    end_current(&store, b);
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
    let id = new_box(&store, "acme/1");
    let admit = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admit).unwrap();
    let up = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started);
    store.apply(up).unwrap();
    add_working_session(&store, id);
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    assert_eq!(session_states(&store, id), ["ended", "ended"]);
}

#[test]
fn rejecting_a_box_ends_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    add_working_session(&store, id);
    let reject = Decision::new(id).lifecycle(BoxLifecycleInput::Rejected {
        reason: "no".into(),
    });
    store.apply(reject).unwrap();
    assert_eq!(session_states(&store, id), ["ended", "ended"]);
}

fn ready_box(store: &crate::store::Store) -> BoxId {
    let id = new_box(store, "acme/1");
    let admit = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admit).unwrap();
    let up = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started);
    store.apply(up).unwrap();
    id
}

fn ended_events(store: &crate::store::Store, id: BoxId) -> Vec<String> {
    store
        .writer()
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
    let conn = store.writer();
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
    let before = store.box_row(id).unwrap().unwrap();
    clock.sleep(std::time::Duration::from_secs(5));
    assert!(!store.end_current_session(id, "again", Some(1)).unwrap());
    assert_eq!(ended_events(&store, id).len(), 1);
    assert_eq!(
        store.box_row(id).unwrap().unwrap().updated_at,
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
    let id = new_box(&store, "acme/1");
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
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert!(row.session.is_quiet());
    assert_eq!(row.idle_since.unwrap().as_millis(), 1_005_000);
}

#[test]
fn ready_box_without_a_session_row_reads_starting() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let conn = store.writer();
    conn.execute("UPDATE events SET session = NULL WHERE box = ?", [id.get()])
        .unwrap();
    conn.execute("DELETE FROM sessions WHERE box = ?", [id.get()])
        .unwrap();
    drop(conn);
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.session, SessionState::Starting);
    assert!(!row.session.is_quiet());
}

fn parent_of(store: &crate::store::Store, session: i64) -> Option<i64> {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT parent_session FROM sessions WHERE id = ?",
            [session],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn open_child_session_records_a_box_or_host_parent() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    let other = new_box(&store, "acme/2");
    let box_parent = store.box_sessions(other).unwrap()[0].id;
    let host = store.open_host_session("/w", None, None).unwrap();
    end_current(&store, id);
    let child = store.open_child_session(id, Some(box_parent)).unwrap();
    assert_eq!(parent_of(&store, child.id), Some(box_parent));
    end_current(&store, id);
    let child = store.open_child_session(id, Some(host)).unwrap();
    assert_eq!(parent_of(&store, child.id), Some(host));
}

#[test]
fn open_session_leaves_the_parent_unset() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    end_current(&store, id);
    let session = store.open_session(id).unwrap();
    assert_eq!(parent_of(&store, session.id), None);
}

#[test]
fn check_parent_accepts_live_box_and_host_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    store
        .check_parent(store.box_sessions(id).unwrap()[0].id)
        .unwrap();
    let host = store.open_host_session("/w", None, None).unwrap();
    store.check_parent(host).unwrap();
}

#[test]
fn check_parent_refuses_unknown_and_ended_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let err = store.check_parent(9999).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(m) if m == "no Session 9999"),
        "{err:?}"
    );
    let host = store.open_host_session("/w", None, None).unwrap();
    store.end_host_session(host).unwrap();
    let err = store.check_parent(host).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(m) if m == &format!("Session {host} has ended")),
        "{err:?}"
    );
}

#[test]
fn open_child_session_refuses_an_ended_parent_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    end_current(&store, id);
    let host = store.open_host_session("/w", None, None).unwrap();
    store.end_host_session(host).unwrap();
    let err = store.open_child_session(id, Some(host)).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(m) if m == &format!("Session {host} has ended")),
        "{err:?}"
    );
    assert_eq!(store.box_sessions(id).unwrap().len(), 1);
}

#[test]
fn open_child_session_keeps_the_box_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "acme/1");
    let set = Fields {
        workspace_id: Field::Set("ws-1".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set)).unwrap();
    end_current(&store, id);
    store.open_child_session(id, None).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.workspace_id.as_deref(), Some("ws-1"));
}
