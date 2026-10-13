//! Tests for session rows, the hook cache and `idle_since`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRecord};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, ProviderId, TaskId, TaskName};

use crate::testing::clock::FakeClock;

use crate::store::Store;

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock.clone()).unwrap();
    (store, clock)
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// Step `id` with `input`.
fn step(store: &Store, id: TaskId, input: TaskInput) {
    store.apply(Decision::new(id).task_input(input)).unwrap();
}

/// A task that is working: admitted, then its box ready.
fn working(store: &Store, name: &str) -> TaskId {
    let id = add(store, name);
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        });
    store.apply(d).unwrap();
    id
}

/// The `idle_since` column of a task.
fn idle_since(store: &Store, id: TaskId) -> Option<i64> {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT idle_since FROM tasks WHERE id = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

/// The state and update time columns of a task.
fn state_cols(store: &Store, id: TaskId) -> (String, i64) {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT state, updated_at FROM tasks WHERE id = ?",
            [id.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
fn live_session_is_the_newest_unended_row() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = add(&store, "t");
    assert_eq!(store.live_session(id).unwrap(), None);
    let first = store.begin_session(id, "s1").unwrap();
    clock.sleep(Duration::from_millis(5));
    let second = store.begin_session(id, "s2").unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    assert_eq!((live.id, live.session_ref.as_str()), (second, "s2"));
    assert_eq!(live.started_at, Ts::from_millis(1_005));
    store.end_session(second, Some(0), Some("dg")).unwrap();
    assert_eq!(store.live_session(id).unwrap().unwrap().id, first);
    store.end_session(first, None, None).unwrap();
    assert_eq!(store.live_session(id).unwrap(), None);
}

#[test]
fn has_session_stays_true_once_a_session_began() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let other = add(&store, "u");
    assert!(!store.has_session(id).unwrap());
    let s = store.begin_session(id, "s1").unwrap();
    assert!(store.has_session(id).unwrap());
    store.end_session(s, Some(0), None).unwrap();
    assert!(store.has_session(id).unwrap());
    assert!(!store.has_session(other).unwrap());
}

#[test]
fn end_session_records_outcome_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = add(&store, "t");
    let s = store.begin_session(id, "s1").unwrap();
    clock.sleep(Duration::from_millis(10));
    store.end_session(s, Some(3), Some("dg")).unwrap();
    clock.sleep(Duration::from_millis(10));
    store.end_session(s, Some(9), None).unwrap();
    let c = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    let row: (i64, i32, String) = c
        .query_row(
            "SELECT ended_at, exit, transcript_digest FROM sessions WHERE id = ?",
            [s],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (1_010, 3, "dg".to_owned()));
}

#[test]
fn session_writes_leave_task_columns_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = working(&store, "t");
    let before = state_cols(&store, id);
    clock.sleep(Duration::from_millis(50));
    let s = store.begin_session(id, "s1").unwrap();
    store.end_session(s, Some(0), None).unwrap();
    assert_eq!(state_cols(&store, id), before);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn begin_session_on_unknown_task_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store.begin_session(TaskId::new(7), "s").unwrap_err();
    assert!(
        matches!(err, crate::store::StoreError::NotFound(_)),
        "{err:?}"
    );
}

#[test]
fn hook_cache_round_trips_every_kind_and_keeps_the_latest() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = add(&store, "t");
    assert_eq!(store.hook(id).unwrap(), None);
    let kinds = [
        HookKind::Starting,
        HookKind::Busy,
        HookKind::Idle,
        HookKind::Blocked,
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        clock.sleep(Duration::from_millis(10));
        let record = HookRecord {
            kind,
            session_id: (i % 2 == 0).then(|| "sess".to_owned()),
            message: (i == 3).then(|| "ok?".to_owned()),
            at: Ts::from_millis(500 + i64::try_from(i).unwrap()),
        };
        store.put_hook(id, &record).unwrap();
        let cache = store.hook(id).unwrap().unwrap();
        assert_eq!(cache.record, record);
        assert_eq!(cache.at, clock.now());
    }
    let c = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    let n: i64 = c
        .query_row("SELECT count(*) FROM hook_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn idle_since_is_set_on_quiet_and_cleared_on_busy() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = working(&store, "t");
    assert_eq!(idle_since(&store, id), None);
    clock.sleep(Duration::from_millis(40));
    step(&store, id, TaskInput::AgentQuiet);
    assert_eq!(idle_since(&store, id), Some(1_040));
    clock.sleep(Duration::from_millis(40));
    store
        .apply(Decision::new(id).set(Fields {
            goal: Field::Set("g".into()),
            ..Fields::default()
        }))
        .unwrap();
    assert_eq!(idle_since(&store, id), Some(1_040));
    step(&store, id, TaskInput::AgentBusy);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn idle_since_clears_when_idle_goes_to_sleep() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = working(&store, "t");
    step(&store, id, TaskInput::AgentQuiet);
    step(&store, id, TaskInput::SleepCmd);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn rejected_step_leaves_idle_since_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = working(&store, "t");
    step(&store, id, TaskInput::AgentQuiet);
    let want = idle_since(&store, id);
    assert!(store
        .apply(Decision::new(id).task_input(TaskInput::Wake))
        .is_err());
    assert_eq!(idle_since(&store, id), want);
}

#[test]
fn hook_cache_reads_a_legacy_blocked_row_as_its_message() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let c = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    c.execute(
        "INSERT INTO hook_cache (task, record, at) VALUES (?1, ?2, 1)",
        rusqlite::params![
            id.get(),
            r#"{"kind":"blocked","waiting_for":"X","session_id":null,"at":5}"#
        ],
    )
    .unwrap();
    let rec = store.hook(id).unwrap().unwrap().record;
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message.as_deref(), Some("X"));
}
