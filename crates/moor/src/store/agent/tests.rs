//! Tests for session rows, the hook cache and `idle_since`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRecord};
use moor_core::config::AgentHolder;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use moor_schema::tables::{Boxes, HookCache, Launches, Sessions};
use sea_query::{Expr, ExprTrait as _, Func, Query};

use crate::store::db::{self, DbExt as _};
use crate::store::select::newest_session;
use crate::testing::clock::FakeClock;

use crate::store::{Store, StoreError};

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock.clone()).unwrap();
    (store, clock)
}

/// Insert a box by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    store.apply(Decision::insert(nt)).unwrap().box_id
}

/// Step `id`'s session with `input`.
fn step(store: &Store, id: BoxId, input: SessionInput) {
    store.apply(Decision::new(id).session(input)).unwrap();
}

/// A box that is working: admitted, then its box ready.
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
        .row(
            Query::select()
                .column(Boxes::IdleSince)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
            |r| r.get(0),
        )
        .unwrap()
}

/// How many hook reports are cached, across all Boxes.
fn hook_rows(store: &Store) -> i64 {
    store
        .reader()
        .unwrap()
        .row(
            Query::select()
                .expr(Func::count(Expr::col(HookCache::Session)))
                .from(HookCache::Table),
            |r| r.get(0),
        )
        .unwrap()
}

/// The state and update time columns of a box.
fn state_cols(store: &Store, id: BoxId) -> (String, i64) {
    store
        .reader()
        .unwrap()
        .row(
            Query::select()
                .columns([Boxes::State, Boxes::UpdatedAt])
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
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
    let first = store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
    clock.sleep(Duration::from_millis(5));
    let second = store
        .begin_session(id, "s2", None, AgentHolder::Hc)
        .unwrap();
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
    store
        .begin_session(id, "s1", Some("p1"), AgentHolder::Hc)
        .unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    assert_eq!(live.pane.as_deref(), Some("p1"));
    let events = store.events(id, 10).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "session_started");
    assert_eq!(last.payload.get("session"), Some(&serde_json::json!("s1")));
}

#[test]
fn a_session_keeps_the_holder_it_began_under() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store
        .begin_session(id, "s1", None, AgentHolder::Tmux)
        .unwrap();
    assert_eq!(
        store.live_session(id).unwrap().unwrap().holder,
        AgentHolder::Tmux
    );
    assert_eq!(store.agent_holder(id).unwrap(), AgentHolder::Tmux);
    store
        .begin_session(id, "s2", None, AgentHolder::Hc)
        .unwrap();
    assert_eq!(
        store.live_session(id).unwrap().unwrap().holder,
        AgentHolder::Hc
    );
    assert_eq!(store.agent_holder(id).unwrap(), AgentHolder::Hc);
}

#[test]
fn a_session_begun_before_holders_were_recorded_is_held_by_tmux() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    assert_eq!(store.agent_holder(id).unwrap(), AgentHolder::Tmux);
    store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
    store.forget_holders(id).unwrap();
    assert_eq!(
        store.live_session(id).unwrap().unwrap().holder,
        AgentHolder::Tmux
    );
    assert_eq!(store.agent_holder(id).unwrap(), AgentHolder::Tmux);
}

#[test]
fn has_session_stays_true_once_a_session_began() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let other = add(&store, "u");
    assert!(!store.has_session(id).unwrap());
    let s = store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
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
    let s = store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
    clock.sleep(Duration::from_millis(10));
    store.end_session(s, Some(3), Some("dg")).unwrap();
    clock.sleep(Duration::from_millis(10));
    store.end_session(s, Some(9), None).unwrap();
    let row: (i64, i32, String) = store
        .reader()
        .unwrap()
        .row(
            Query::select()
                .columns([
                    Launches::EndedAt,
                    Launches::Exit,
                    Launches::TranscriptDigest,
                ])
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Id).eq(s)),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (1_010, 3, "dg".to_owned()));
}

#[test]
fn session_writes_leave_box_columns_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = working(&store, "t");
    let before = state_cols(&store, id);
    clock.sleep(Duration::from_millis(50));
    let s = store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
    store.end_session(s, Some(0), None).unwrap();
    assert_eq!(state_cols(&store, id), before);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn begin_session_on_unknown_box_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store
        .begin_session(BoxId::new(7), "s", None, AgentHolder::Hc)
        .unwrap_err();
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
        HookKind::Starting { launch: None },
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
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(500 + i64::try_from(i).unwrap()),
            turn: None,
        };
        store.put_hook(id, &record).unwrap();
        let cache = store.hook(id).unwrap().unwrap();
        assert_eq!(cache.record, record);
        assert_eq!(cache.at, clock.now());
    }
    assert_eq!(hook_rows(&store), 1);
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
            prompt: Field::Set("g".into()),
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
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Sleep {
            auto: false,
            reason: None,
        }))
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
    let first = Query::select()
        .column(Sessions::Id)
        .from(Sessions::Table)
        .and_where(Expr::col(Sessions::Box).eq(id.get()))
        .and_where(Expr::col(Sessions::Ordinal).eq(1))
        .take();
    store
        .writer()
        .run(
            Query::insert()
                .into_table(HookCache::Table)
                .columns([HookCache::Session, HookCache::Record, HookCache::At])
                .values_panic([
                    db::sub(first),
                    r#"{"kind":"blocked","waiting_for":"X","session_id":null,"at":5}"#.into(),
                    1.into(),
                ]),
        )
        .unwrap();
    let rec = store.hook(id).unwrap().unwrap().record;
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message.as_deref(), Some("X"));
}

fn set_session_state(store: &Store, id: BoxId, state: &str) {
    store
        .writer()
        .run(
            Query::update()
                .table(Sessions::Table)
                .value(Sessions::State, state)
                .and_where(Expr::col(Sessions::Id).eq(db::sub(newest_session(id)))),
        )
        .unwrap();
}

#[test]
fn begin_session_refuses_an_ended_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    set_session_state(&store, id, "ended");
    let err = store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert_eq!(store.live_session(id).unwrap(), None);
}

#[test]
fn begin_session_allows_a_starting_or_working_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
    set_session_state(&store, id, "working");
    store
        .begin_session(id, "s2", None, AgentHolder::Hc)
        .unwrap();
}

#[test]
fn begin_session_records_the_current_box_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    set_session_state(&store, id, "ended");
    let second = store.open_session(id).unwrap();
    store
        .begin_session(id, "s1", None, AgentHolder::Hc)
        .unwrap();
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
        options: Vec::new(),
        doing: None,
        at: Ts::from_millis(5),
        turn: None,
    };
    store.put_hook(id, &record(HookKind::Busy)).unwrap();
    set_session_state(&store, id, "ended");
    let second = store.open_session(id).unwrap();
    assert_eq!(store.hook(id).unwrap(), None);
    store.put_hook(id, &record(HookKind::Idle)).unwrap();
    let conn = store.writer();
    let cached: i64 = conn
        .row(
            Query::select()
                .column(HookCache::Session)
                .from(HookCache::Table)
                .and_where(Expr::col(HookCache::Session).eq(second.id)),
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    let total = hook_rows(&store);
    assert_eq!((cached, total), (second.id, 2));
    assert_eq!(store.hook(id).unwrap().unwrap().record.kind, HookKind::Idle);
}

#[test]
fn a_hook_report_keeps_its_doing_line_through_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = working(&store, "t");
    let record = HookRecord {
        kind: HookKind::Busy,
        session_id: None,
        message: None,
        options: Vec::new(),
        doing: Some("Bash: cargo test".to_owned()),
        at: Ts::from_millis(900),
        turn: None,
    };
    store.put_hook(id, &record).unwrap();
    assert_eq!(store.hook(id).unwrap().unwrap().record, record);
}

#[test]
fn a_waiting_hook_keeps_what_it_waits_on() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = add(&store, "t");
    let record = HookRecord {
        kind: HookKind::Waiting {
            on: vec!["gh run watch".to_owned(), "tests".to_owned()],
        },
        session_id: None,
        message: Some("Watching.".to_owned()),
        options: Vec::new(),
        doing: None,
        at: Ts::from_millis(500),
        turn: None,
    };
    store.put_hook(id, &record).unwrap();
    assert_eq!(store.hook(id).unwrap().unwrap().record, record);
}
