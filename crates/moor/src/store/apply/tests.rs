//! Tests for the writer: name collisions on insert.

use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::TaskInput;
use moor_core::BoxName;

use crate::store::tests::open;
use crate::store::StoreError;

/// A new-task decision.
fn insert(name: &str) -> Decision {
    Decision::insert(NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
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
    let id = store.apply(insert("dup")).unwrap().task;
    store
        .apply(Decision::new(id).task_input(TaskInput::Close))
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
    let id = store.apply(insert("b")).unwrap().task;
    let admit = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(moor_core::domain::boxes::BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(moor_core::ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admit).unwrap();
    let up = TaskInput::BoxReady {
        setup: moor_core::domain::task::SetupOutcome::Ok,
    };
    store.apply(Decision::new(id).task_input(up)).unwrap();
    id
}

#[test]
fn insert_records_the_branch_ref() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let mut d = insert("b");
    d.set.branch = Field::Set("feat/x".into());
    let id = store.apply(d).unwrap().task;
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

#[test]
fn box_steps_between_working_and_idle() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    assert_eq!(states(&store, id), ("ready".into(), "working".into()));
    store
        .apply(Decision::new(id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    assert_eq!(states(&store, id), ("ready".into(), "idle".into()));
    store
        .apply(Decision::new(id).task_input(TaskInput::AgentBusy))
        .unwrap();
    assert_eq!(states(&store, id), ("ready".into(), "working".into()));
}

#[test]
fn sleep_is_refused_while_another_session_works() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready(&store);
    store
        .apply(Decision::new(id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .writer()
        .unwrap()
        .execute(
            "INSERT INTO sessions (box, ordinal, state, started_at) VALUES (?, 2, 'working', 0)",
            [id.get()],
        )
        .unwrap();
    let err = store
        .apply(Decision::new(id).task_input(TaskInput::SleepDue))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::SleepBlocked { busy: 1 }),
        "{err:?}"
    );
    assert_eq!(states(&store, id).0, "ready");
}
