//! Tests for the writer: name collisions on insert.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::BoxName;

use crate::store::tests::open;
use crate::store::StoreError;

/// A new-box decision.
fn insert(name: &str) -> Decision {
    Decision::insert(NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    })
}

#[test]
fn open_name_collision_is_name_taken() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    store.apply(insert("dup")).unwrap();
    let err = store.apply(insert("dup")).unwrap_err();
    assert!(matches!(err, StoreError::NameTaken(_)), "{err:?}");
}

#[test]
fn closed_name_can_be_reused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store.apply(insert("dup")).unwrap().box_id;
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    store.apply(insert("dup")).unwrap();
}

/// Box columns and primary Session column, as stored.
fn states(store: &crate::store::Store, id: moor_core::BoxId) -> (String, String) {
    let conn = store.writer().unwrap();
    let b: String = conn
        .query_row("SELECT state FROM boxes WHERE id = ?", [id.get()], |r| {
            r.get(0)
        })
        .unwrap();
    let s: String = conn
        .query_row(
            "SELECT state FROM sessions WHERE box = ? AND ordinal = 1",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    (b, s)
}

/// Move a fresh Box to Ready with a working primary Session.
fn ready(store: &crate::store::Store) -> moor_core::BoxId {
    let id = store.apply(insert("b")).unwrap().box_id;
    let admit = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(moor_core::domain::boxes::BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(moor_core::ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admit).unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: moor_core::domain::box_state::SetupOutcome::Ok,
                })
                .session(SessionInput::Started),
        )
        .unwrap();
    id
}

#[test]
fn insert_records_the_branch_ref() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let mut d = insert("b");
    d.set.branch = Field::Set("feat/x".into());
    let id = store.apply(d).unwrap().box_id;
    assert_eq!(store.box_refs(id, "branch").unwrap(), ["feat/x"]);
}

#[test]
fn failed_insert_leaves_no_box_and_no_branch_ref() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let mut d = insert("b").box_input(moor_core::domain::boxes::BoxInput::Requested);
    d.set.branch = Field::Set("feat/x".into());
    let err = store.apply(d).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(m) if m == "new box needs box_provider"),
        "{err:?}"
    );
    for table in ["box_refs", "boxes"] {
        let n: i64 = store
            .writer()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table} kept rows after a failed insert");
    }
}

/// Force the highest-ordinal Session into `state`.
fn set_current_state(store: &crate::store::Store, id: moor_core::BoxId, state: &str) {
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = ?2 WHERE box = ?1 AND ordinal = (SELECT MAX(ordinal) FROM sessions WHERE box = ?1)",
            rusqlite::params![id.get(), state],
        )
        .unwrap();
}

#[test]
fn box_steps_between_working_and_idle() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    assert_eq!(states(&store, id), ("ready".into(), "working".into()));
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    assert_eq!(states(&store, id), ("ready".into(), "idle".into()));
    store
        .apply(Decision::new(id).session(SessionInput::AgentBusy))
        .unwrap();
    assert_eq!(states(&store, id), ("ready".into(), "working".into()));
}

#[test]
fn sleep_is_refused_while_another_session_works() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    store
        .writer()
        .unwrap()
        .execute(
            "INSERT INTO sessions (box, ordinal, state, started_at) VALUES (?, 2, 'idle', 0)",
            [id.get()],
        )
        .unwrap();
    let err = store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Sleep { auto: true }))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::SleepBlocked { busy: 1 }),
        "{err:?}"
    );
    assert_eq!(states(&store, id).0, "ready");
}

#[test]
fn step_updates_only_the_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = 'ended' WHERE box = ? AND ordinal = 1",
            [id.get()],
        )
        .unwrap();
    store.open_session(id).unwrap();
    set_current_state(&store, id, "working");
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    let states: Vec<String> = store
        .box_sessions(id)
        .unwrap()
        .iter()
        .map(|s| s.state.as_str().to_owned())
        .collect();
    assert_eq!(states, ["ended", "idle"]);
}

#[test]
fn box_row_reports_the_current_session_state() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = 'ended' WHERE box = ? AND ordinal = 1",
            [id.get()],
        )
        .unwrap();
    store.open_session(id).unwrap();
    set_current_state(&store, id, "working");
    let conn = store.writer().unwrap();
    let row = crate::store::load_box(&conn, id).unwrap().unwrap();
    drop(conn);
    assert_eq!(row.session.as_str(), "working");
}

#[test]
fn ended_other_sessions_do_not_block_sleep() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = 'ended' WHERE box = ? AND ordinal = 1",
            [id.get()],
        )
        .unwrap();
    store.open_session(id).unwrap();
    set_current_state(&store, id, "working");
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Sleep { auto: true }))
        .unwrap();
    assert_eq!(states(&store, id).0, "asleep");
}

#[test]
fn insert_writes_parent_session_on_the_first_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let parent = store.open_host_session("/w", None, None).unwrap();
    let child = store
        .apply(insert("child").parent_session(Some(parent)))
        .unwrap()
        .box_id;
    let plain = store.apply(insert("plain")).unwrap().box_id;
    let conn = store.writer().unwrap();
    let get = |id: moor_core::BoxId| -> Option<i64> {
        conn.query_row(
            "SELECT parent_session FROM sessions WHERE box = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(get(child), Some(parent));
    assert_eq!(get(plain), None);
}

#[test]
fn insert_with_an_ended_parent_session_writes_no_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let parent = store.open_host_session("/w", None, None).unwrap();
    store.end_host_session(parent).unwrap();
    let err = store
        .apply(insert("child").parent_session(Some(parent)))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    let boxes: i64 = store
        .writer()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM boxes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(boxes, 0);
}

/// Kinds and payloads of every event on the Box, in order.
fn event_trail(
    store: &crate::store::Store,
    id: moor_core::BoxId,
) -> Vec<(String, serde_json::Value)> {
    store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.payload))
        .collect()
}

#[test]
fn closing_a_ready_box_records_the_box_state_as_from() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    let trail = event_trail(&store, id);
    let closed = trail.iter().find(|(k, _)| k == "closed");
    assert_eq!(
        closed.map(|(_, p)| p),
        Some(&serde_json::json!({"from": "ready"})),
        "{trail:?}"
    );
}

#[test]
fn separate_close_inputs_match_the_combined_close() {
    use moor_core::domain::box_state::BoxLifecycleInput;
    use moor_core::domain::session_state::SessionInput;

    let combined_dir = tempfile::tempdir().unwrap();
    let (combined, _clock) = open(combined_dir.path());
    let a = ready(&combined);
    combined
        .apply(
            Decision::new(a)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();

    let split_dir = tempfile::tempdir().unwrap();
    let (split, _clock) = open(split_dir.path());
    let b = ready(&split);
    split
        .apply(
            Decision::new(b)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();

    assert_eq!(event_trail(&combined, a), event_trail(&split, b));
}

#[test]
fn setting_up_after_admission_records_checking_as_from() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = store.apply(insert("b")).unwrap().box_id;
    let admit = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(moor_core::domain::boxes::BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(moor_core::ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admit).unwrap();
    let trail = event_trail(&store, id);
    let setting_up = trail.iter().find(|(k, _)| k == "setting_up");
    assert_eq!(
        setting_up.map(|(_, p)| p),
        Some(&serde_json::json!({"from": "checking"})),
        "{trail:?}"
    );
}

/// Sets Box `id`'s branch to `branch`.
fn on_branch(id: moor_core::BoxId, branch: &str) -> Decision {
    Decision::new(id).set(Fields {
        branch: Field::Set(branch.to_owned()),
        ..Fields::default()
    })
}

#[test]
fn two_open_boxes_cannot_share_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = store.apply(insert("a")).unwrap().box_id;
    store.apply(on_branch(a, "moor/x")).unwrap();
    let b = store.apply(insert("b")).unwrap().box_id;
    let err = store.apply(on_branch(b, "moor/x")).unwrap_err();
    assert!(
        matches!(&err, StoreError::BranchTaken(x) if x == "moor/x"),
        "{err:?}"
    );
    let raced = insert("c").set(Fields {
        branch: Field::Set("moor/x".to_owned()),
        ..Fields::default()
    });
    let err = store.apply(raced).unwrap_err();
    assert!(matches!(err, StoreError::BranchTaken(_)), "{err:?}");
    assert!(store
        .open_box_by_name(&BoxName::parse_allow_bare("c").unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn the_branch_of_a_closed_box_can_be_taken_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = store.apply(insert("a")).unwrap().box_id;
    store.apply(on_branch(a, "moor/x")).unwrap();
    store
        .apply(
            Decision::new(a)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    let b = store.apply(insert("b")).unwrap().box_id;
    store.apply(on_branch(b, "moor/x")).unwrap();
}
