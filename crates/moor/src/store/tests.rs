//! Tests for the store: open, `apply`, atomicity, WAL readers, runs.
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{
    Decision, ExtraEvent, Field, Fields, Intent, IntentChange, NewTask,
};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, ProviderId, RunId, TaskId, TaskName};

use crate::testing::clock::FakeClock;

use super::apply::Applied;
use super::reads::live_holders_in;
use super::{NewRun, RunOutcome, Store, StoreError};

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.join("state").join("moor.db"), clock.clone()).unwrap();
    (store, clock)
}

/// A new-task decision.
fn insert(name: &str) -> Decision {
    Decision::insert(NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    })
}

/// Insert a task and return its id.
fn new_task(store: &Store, name: &str) -> TaskId {
    store.apply(insert(name)).unwrap().task
}

/// Fields naming the box provider.
fn provider() -> Fields {
    Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    }
}

/// First event id of an apply.
fn eid(a: &Applied) -> i64 {
    *a.events.first().unwrap()
}

/// Event kinds of a task in id order.
fn kinds(store: &Store, id: TaskId) -> Vec<String> {
    store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

/// Count rows of a table through a raw connection.
fn count(path: &Path, table: &str) -> i64 {
    let c = rusqlite::Connection::open(path).unwrap();
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn open_creates_wal_file_with_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    drop(open(dir.path()));
    assert!(path.exists());
    let raw = rusqlite::Connection::open(&path).unwrap();
    let mode: String = raw
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let v: String = raw
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(v, "1");
    drop(raw);
    let (store, _) = open(dir.path());
    assert!(store.open_tasks().unwrap().is_empty());
}

#[test]
fn open_rejects_mismatched_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("moor.db");
    drop(open(dir.path()));
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute("UPDATE meta SET v = '99' WHERE k = 'schema_version'", [])
        .unwrap();
    drop(raw);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let err = Store::open(&path, clock).err().unwrap();
    assert!(matches!(err, StoreError::Schema { .. }), "{err:?}");
}

#[test]
fn full_lifecycle_rows_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    assert_eq!(store.task(id).unwrap().unwrap().state, TaskState::Checking);

    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(provider());
    let a = store.apply(d).unwrap();
    assert_eq!(
        (a.task_state, a.box_state),
        (TaskState::SettingUp, Some(BoxState::Starting))
    );

    let handle = BoxHandle::new("h1");
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: handle.clone(),
        });
    store.apply(d).unwrap();
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Working);
    let b = row.boxed.unwrap();
    assert_eq!((b.state, b.handle), (BoxState::Ready, Some(handle)));

    let d = Decision::new(id)
        .task_input(TaskInput::Close)
        .box_input(BoxInput::Released)
        .event(ExtraEvent::Copied {
            from: "a".into(),
            to: "b".into(),
        });
    let a = store.apply(d).unwrap();
    assert_eq!(a.events.len(), 3);
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(
        (row.state, row.boxed.unwrap().state),
        (TaskState::Closed, BoxState::Deleted)
    );
    let want = [
        "started",
        "setting_up",
        "box_requested",
        "working",
        "box_ready",
        "closed",
        "box_deleted",
        "copied",
    ];
    assert_eq!(kinds(&store, id), want);
}

#[test]
fn new_task_with_illegal_box_input_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let d = insert("t")
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        })
        .set(provider());
    let err = store.apply(d).unwrap_err();
    assert!(matches!(err, StoreError::Illegal(_)), "{err:?}");
    let path = dir.path().join("state").join("moor.db");
    assert_eq!((count(&path, "tasks"), count(&path, "events")), (0, 0));
}

#[test]
fn illegal_task_step_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    let before = store.events(id, 100).unwrap().len();
    let set = Fields {
        goal: Field::Set("g".into()),
        ..provider()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Wake)
        .box_input(BoxInput::Requested)
        .set(set);
    assert!(matches!(
        store.apply(d).unwrap_err(),
        StoreError::Illegal(_)
    ));
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(
        (row.state, row.goal, row.boxed),
        (TaskState::Checking, None, None)
    );
    assert_eq!(store.events(id, 100).unwrap().len(), before);
}

#[test]
fn unknown_existing_id_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store
        .apply(Decision::new(TaskId::new(42)).task_input(TaskInput::Close))
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");
}

#[test]
fn readers_are_not_blocked_by_an_open_writer() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    let path = dir.path().join("state").join("moor.db");
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "BEGIN EXCLUSIVE; UPDATE tasks SET goal = 'dirty'; \
         INSERT INTO tasks (name, repo, state, created_at, updated_at) VALUES ('x', 'r', 'checking', 0, 0); \
         INSERT INTO events (task, kind, payload, ts) VALUES (1, 'busy', '{}', 0);",
    )
    .unwrap();
    let start = Instant::now();
    assert_eq!(store.task(id).unwrap().unwrap().goal, None);
    assert_eq!(store.open_tasks().unwrap().len(), 1);
    assert_eq!(store.events(id, 100).unwrap().len(), 1);
    assert!(start.elapsed() < Duration::from_secs(1));
    raw.execute_batch("COMMIT").unwrap();
    assert_eq!(store.open_tasks().unwrap().len(), 2);
    store
        .apply(Decision::new(id).task_input(TaskInput::Close))
        .unwrap();
}

#[test]
fn box_fields_without_a_box_roll_back_the_task_step() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    let before = store.task(id).unwrap().unwrap();
    assert_eq!(
        (before.state, before.boxed.is_none()),
        (TaskState::Checking, true)
    );
    let events_before = kinds(&store, id);
    // step_task (Checking -> SettingUp) and write_task both succeed; only
    // write_box fails, so the task UPDATE must be rolled back.
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .set(Fields {
            box_workdir: Field::Set("/w".into()),
            ..Fields::default()
        });
    let err = store.apply(d).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    let after = store.task(id).unwrap().unwrap();
    assert_eq!(after, before);
    assert_eq!(after.state, TaskState::Checking);
    assert_eq!(kinds(&store, id), events_before);
    let path = dir.path().join("state").join("moor.db");
    assert_eq!(
        count(&path, "events"),
        i64::try_from(events_before.len()).unwrap()
    );
}

#[test]
fn event_ids_increase_and_ts_follows_clock() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = new_task(&store, "t");
    clock.sleep(Duration::from_millis(500));
    let a = store
        .apply(Decision::new(id).event(ExtraEvent::SessionStarted {
            session: "s".into(),
        }))
        .unwrap();
    let b = store
        .apply(Decision::new(id).event(ExtraEvent::Copied {
            from: "a".into(),
            to: "b".into(),
        }))
        .unwrap();
    let c = store
        .apply(Decision::new(id).event(ExtraEvent::RunStarted { run: RunId::new(1) }))
        .unwrap();
    let (ia, ib, ic) = (eid(&a), eid(&b), eid(&c));
    assert!(ia < ib && ib < ic);
    let all = store.events(id, 100).unwrap();
    assert!(all.iter().zip(all.iter().skip(1)).all(|(x, y)| x.id < y.id));
    assert_eq!(all.first().map(|e| e.ts), Some(Ts::from_millis(1_000_000)));
    assert_eq!(all.last().map(|e| e.ts), Some(Ts::from_millis(1_000_500)));
    let last = store.events(id, 2).unwrap();
    assert_eq!(last.iter().map(|e| e.id).collect::<Vec<_>>(), vec![ib, ic]);
}

#[test]
fn setup_failed_at_set_and_clear_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    let set = |f| {
        Decision::new(id).set(Fields {
            setup_failed_at: f,
            ..Fields::default()
        })
    };
    store.apply(set(Field::Set(4_242))).unwrap();
    assert_eq!(
        store.task(id).unwrap().unwrap().setup_failed_at,
        Some(Ts::from_millis(4_242))
    );
    store.apply(set(Field::Clear)).unwrap();
    assert_eq!(store.task(id).unwrap().unwrap().setup_failed_at, None);
}

/// A task with a Ready box, as after provisioning.
fn task_with_ready_box(store: &Store) -> TaskId {
    let id = new_task(store, "t");
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(provider()),
        )
        .unwrap();
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::BoxReady {
                    setup: SetupOutcome::Ok,
                })
                .box_input(BoxInput::Acquired {
                    handle: BoxHandle::new("h1"),
                }),
        )
        .unwrap();
    id
}

#[test]
fn fresh_box_has_no_sleep_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = task_with_ready_box(&store);
    assert_eq!(
        store.task(id).unwrap().unwrap().boxed.unwrap().sleep_reason,
        None
    );
}

#[test]
fn sleep_reason_is_set_without_changing_states() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = task_with_ready_box(&store);
    let before = store.task(id).unwrap().unwrap();
    store
        .apply(Decision::new(id).set(Fields {
            box_sleep_reason: Field::Set("no wake".into()),
            ..Fields::default()
        }))
        .unwrap();
    let after = store.task(id).unwrap().unwrap();
    let b = after.boxed.unwrap();
    assert_eq!(b.sleep_reason.as_deref(), Some("no wake"));
    assert_eq!(
        (after.state, b.state),
        (before.state, before.boxed.unwrap().state)
    );
}

#[test]
fn sleep_reason_clears_in_the_same_apply_as_a_box_input() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = task_with_ready_box(&store);
    let reason = |f| {
        Decision::new(id).set(Fields {
            box_sleep_reason: f,
            ..Fields::default()
        })
    };
    store.apply(reason(Field::Set("no wake".into()))).unwrap();
    store
        .apply(reason(Field::Clear).box_input(BoxInput::Sleep))
        .unwrap();
    let b = store.task(id).unwrap().unwrap().boxed.unwrap();
    assert_eq!((b.state, b.sleep_reason), (BoxState::Asleep, None));
}

#[test]
fn intent_set_clear_and_run_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    store
        .apply(Decision::new(id).intent(IntentChange::Set(Intent::Provisioning { pid: 7 })))
        .unwrap();
    assert_eq!(
        store.task(id).unwrap().unwrap().intent,
        Some(Intent::Provisioning { pid: 7 })
    );
    store
        .apply(Decision::new(id).intent(IntentChange::Clear))
        .unwrap();
    assert_eq!(store.task(id).unwrap().unwrap().intent, None);

    let run = store
        .begin_run(NewRun {
            task: id,
            argv: vec!["ls".into()],
            record: None,
            sha: None,
            sync: "tree".into(),
            pid: 9,
        })
        .unwrap();
    assert_eq!(
        store.task(id).unwrap().unwrap().intent,
        Some(Intent::Running { run, pid: 9 })
    );
    store
        .finish_run(run, RunOutcome::ended(Some(3), false))
        .unwrap();
    assert_eq!(store.task(id).unwrap().unwrap().intent, None);
    let events = store.events(id, 100).unwrap();
    let ran = events.last().unwrap();
    assert_eq!(ran.kind, "ran");
    assert_eq!(
        ran.payload,
        serde_json::json!({"run": run.get(), "exit": 3, "interrupted": false})
    );
    assert_eq!(
        events.iter().rev().nth(1).map(|e| e.kind.as_str()),
        Some("run_started")
    );
    let err = store
        .finish_run(RunId::new(999), RunOutcome::ended(None, true))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn open_tasks_excludes_closed_and_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let live = new_task(&store, "a");
    let closed = new_task(&store, "b");
    let rejected = new_task(&store, "c");
    store
        .apply(Decision::new(closed).task_input(TaskInput::Close))
        .unwrap();
    store
        .apply(Decision::new(rejected).task_input(TaskInput::Rejected {
            reason: "no".into(),
        }))
        .unwrap();
    let open_ids: Vec<_> = store
        .open_tasks()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(open_ids, vec![live]);
}

/// Claim the only slot of provider `p` for `id`, or queue the task when it is taken.
fn claim_last_slot(store: &Store, id: TaskId) -> Option<Applied> {
    let provider = ProviderId::new("p");
    store
        .apply_admitting(&provider, |holders| {
            let d = Decision::new(id);
            Some(if holders.is_empty() {
                d.task_input(TaskInput::Admitted)
                    .box_input(BoxInput::Requested)
                    .set(self::provider())
            } else {
                d.task_input(TaskInput::AtCapacity { max_boxes: 1 })
            })
        })
        .unwrap()
}

#[test]
fn two_tasks_racing_for_the_last_slot_cannot_both_be_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (a, b) = (new_task(&store, "a"), new_task(&store, "b"));
    // A second store on the same file stands in for a second `moor start` process:
    // only SQLite's write lock, not the in-process writer mutex, can separate the two.
    let rival_store = Store::open(
        &dir.path().join("state").join("moor.db"),
        Arc::new(FakeClock::new(Ts::from_millis(1_000_000))),
    )
    .unwrap();
    let provider = ProviderId::new("p");
    let (lock_held, rival_may_claim) = std::sync::mpsc::channel::<()>();
    let admitted = std::thread::scope(|s| {
        let rival_store = &rival_store;
        let rival = s.spawn(move || {
            rival_may_claim.recv().unwrap();
            claim_last_slot(rival_store, b)
        });
        let first = store
            .apply_admitting(&provider, |holders| {
                assert!(holders.is_empty());
                lock_held.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(300));
                Some(
                    Decision::new(a)
                        .task_input(TaskInput::Admitted)
                        .box_input(BoxInput::Requested)
                        .set(self::provider()),
                )
            })
            .unwrap();
        let second = rival.join().unwrap();
        [first, second]
    });
    let states = [a, b].map(|id| store.task(id).unwrap().unwrap().state);
    assert_eq!(admitted.map(|x| x.is_some()), [true, true]);
    assert_eq!(states, [TaskState::SettingUp, TaskState::WaitingForBox]);
    assert_eq!(
        live_holders_in(&store.reader().unwrap(), &provider)
            .unwrap()
            .len(),
        1
    );
}
