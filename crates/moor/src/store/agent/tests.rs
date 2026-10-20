//! Tests for session rows, the hook cache and `idle_since`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRecord};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use crate::testing::clock::FakeClock;

use crate::store::{Store, StoreError};

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock.clone()).unwrap();
    (store, clock)
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// Step `id`'s session with `input`.
fn step(store: &Store, id: BoxId, input: SessionInput) {
    store.apply(Decision::new(id).session(input)).unwrap();
}

/// A task that is working: admitted, then its box ready.
fn working(store: &Store, name: &str) -> BoxId {
    let id = add(store, name);
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        });
    store.apply(d).unwrap();
    id
}

/// The `idle_since` column of a box.
fn idle_since(store: &Store, id: BoxId) -> Option<i64> {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT idle_since FROM boxes WHERE id = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

/// The state and update time columns of a box.
fn state_cols(store: &Store, id: BoxId) -> (String, i64) {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT state, updated_at FROM boxes WHERE id = ?",
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
    let first = store.begin_session(id, "s1", None).unwrap();
    clock.sleep(Duration::from_millis(5));
    let second = store.begin_session(id, "s2", None).unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    assert_eq!((live.id, live.session_ref.as_str()), (second, "s2"));
    assert_eq!(live.started_at, Ts::from_millis(1_005));
    store.end_session(second, Some(0), Some("dg")).unwrap();
    assert_eq!(store.live_session(id).unwrap().unwrap().id, first);
    store.end_session(first, None, None).unwrap();
    assert_eq!(store.live_session(id).unwrap(), None);
}

#[test]
fn begin_session_keeps_the_pane_and_appends_session_started() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store.begin_session(id, "s1", Some("p1")).unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    assert_eq!(live.pane.as_deref(), Some("p1"));
    let events = store.events(id, 10).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "session_started");
    assert_eq!(last.payload.get("session"), Some(&serde_json::json!("s1")));
}

#[test]
fn has_session_stays_true_once_a_session_began() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let other = add(&store, "u");
    assert!(!store.has_session(id).unwrap());
    let s = store.begin_session(id, "s1", None).unwrap();
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
    let s = store.begin_session(id, "s1", None).unwrap();
    clock.sleep(Duration::from_millis(10));
    store.end_session(s, Some(3), Some("dg")).unwrap();
    clock.sleep(Duration::from_millis(10));
    store.end_session(s, Some(9), None).unwrap();
    let c = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    let row: (i64, i32, String) = c
        .query_row(
            "SELECT ended_at, exit, transcript_digest FROM launches WHERE id = ?",
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
    let s = store.begin_session(id, "s1", None).unwrap();
    store.end_session(s, Some(0), None).unwrap();
    assert_eq!(state_cols(&store, id), before);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn begin_session_on_unknown_task_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store.begin_session(BoxId::new(7), "s", None).unwrap_err();
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
    step(&store, id, SessionInput::AgentQuiet);
    assert_eq!(idle_since(&store, id), Some(1_040));
    clock.sleep(Duration::from_millis(40));
    store
        .apply(Decision::new(id).set(Fields {
            goal: Field::Set("g".into()),
            ..Fields::default()
        }))
        .unwrap();
    assert_eq!(idle_since(&store, id), Some(1_040));
    step(&store, id, SessionInput::AgentBusy);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn idle_since_clears_when_idle_goes_to_sleep() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = working(&store, "t");
    step(&store, id, SessionInput::AgentQuiet);
    store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Sleep { auto: false }))
        .unwrap();
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn rejected_step_leaves_idle_since_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = working(&store, "t");
    step(&store, id, SessionInput::AgentQuiet);
    let want = idle_since(&store, id);
    assert!(store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Wake)
                .session(SessionInput::AgentBusy)
        )
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
        "INSERT INTO hook_cache (session, record, at) \
         VALUES ((SELECT id FROM sessions WHERE box = ?1 AND ordinal = 1), ?2, 1)",
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

fn set_session_state(store: &Store, id: BoxId, state: &str) {
    store
        .writer()
        .unwrap()
        .execute(
            "UPDATE sessions SET state = ? WHERE box = ? AND ordinal = (SELECT MAX(ordinal) FROM sessions WHERE box = ?)",
            (state, id.get(), id.get()),
        )
        .unwrap();
}

#[test]
fn begin_session_refuses_an_ended_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    set_session_state(&store, id, "ended");
    let err = store.begin_session(id, "s1", None).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert_eq!(store.live_session(id).unwrap(), None);
}

#[test]
fn begin_session_allows_a_starting_or_working_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store.begin_session(id, "s1", None).unwrap();
    set_session_state(&store, id, "working");
    store.begin_session(id, "s2", None).unwrap();
}

#[test]
fn begin_session_records_the_current_box_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    set_session_state(&store, id, "ended");
    let second = store.open_session(id).unwrap();
    store.begin_session(id, "s1", None).unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    assert_eq!(live.session, Some(second.id));
}

#[test]
fn hook_follows_the_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let record = |kind| HookRecord {
        kind,
        session_id: None,
        message: None,
        at: Ts::from_millis(5),
    };
    store.put_hook(id, &record(HookKind::Busy)).unwrap();
    set_session_state(&store, id, "ended");
    let second = store.open_session(id).unwrap();
    assert_eq!(store.hook(id).unwrap(), None);
    store.put_hook(id, &record(HookKind::Idle)).unwrap();
    let conn = store.writer().unwrap();
    let cached: i64 = conn
        .query_row(
            "SELECT session FROM hook_cache WHERE session = ?",
            [second.id],
            |r| r.get(0),
        )
        .unwrap();
    let total: i64 = conn
        .query_row("SELECT count(*) FROM hook_cache", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert_eq!((cached, total), (second.id, 2));
    assert_eq!(store.hook(id).unwrap().unwrap().record.kind, HookKind::Idle);
}
