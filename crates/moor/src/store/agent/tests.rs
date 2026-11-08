//! Tests for session rows, agent reads and `idle_since`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use moor_schema::tables::{Boxes, Launches, Sessions};
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{self, DbExt as _};
use crate::store::select::newest_session;
use crate::testing::clock::FakeClock;

use crate::store::{Store, StoreError};

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.join("moor.db"), clock.clone());
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
fn a_box_with_no_session_or_a_herdr_core_session_may_be_reached() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store.require_hc_holder(id, "t").unwrap();
    store.begin_session(id, "s1", None).unwrap();
    store.require_hc_holder(id, "t").unwrap();
}

#[test]
fn a_box_whose_session_another_holder_kept_names_the_box_and_says_to_recreate_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store.begin_session(id, "s1", None).unwrap();
    for held in [Some("tmux"), None] {
        store.set_holders(id, held).unwrap();
        let e = store.require_hc_holder(id, "t").unwrap_err().to_string();
        assert!(e.contains("Box t"), "{e}");
        assert!(e.contains("close the Box and recreate it"), "{e}");
    }
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
    let s = store.begin_session(id, "s1", None).unwrap();
    store.end_session(s, Some(0), None).unwrap();
    assert_eq!(state_cols(&store, id), before);
    assert_eq!(idle_since(&store, id), None);
}

#[test]
fn begin_session_on_unknown_box_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store.begin_session(BoxId::new(7), "s", None).unwrap_err();
    assert!(
        matches!(err, crate::store::StoreError::NotFound(_)),
        "{err:?}"
    );
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
fn an_agent_is_unread_only_after_a_read_older_than_two_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let launch = store.begin_session(id, "s1", None).unwrap();
    let read_at = Ts::from_millis(10_000);
    store.note_agent_read(launch, read_at).unwrap();
    let at = |secs: i64| Ts::from_millis(10_000 + secs * 1_000);
    assert!(!store.agent_unread(id, at(0)).unwrap());
    assert!(!store.agent_unread(id, at(119)).unwrap());
    assert!(store.agent_unread(id, at(120)).unwrap());
    assert!(store.agent_unread(id, at(600)).unwrap());
    store.note_agent_read(launch, at(600)).unwrap();
    assert!(!store.agent_unread(id, at(601)).unwrap());
}

#[test]
fn an_agent_never_read_is_not_unread() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    store.begin_session(id, "s1", None).unwrap();
    assert!(!store.agent_unread(id, Ts::from_millis(10_000_000)).unwrap());
}

#[test]
fn a_box_with_no_live_session_is_not_unread() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    assert!(!store.agent_unread(id, Ts::from_millis(10_000_000)).unwrap());
    let launch = store.begin_session(id, "s1", None).unwrap();
    store
        .note_agent_read(launch, Ts::from_millis(1_000))
        .unwrap();
    store.end_session(launch, Some(0), None).unwrap();
    assert!(!store.agent_unread(id, Ts::from_millis(10_000_000)).unwrap());
}

#[test]
fn a_read_of_an_ended_launch_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = add(&store, "t");
    let first = store.begin_session(id, "s1", None).unwrap();
    store.end_session(first, Some(0), None).unwrap();
    store
        .note_agent_read(first, Ts::from_millis(1_000))
        .unwrap();
    set_session_state(&store, id, "working");
    store.begin_session(id, "s2", None).unwrap();
    assert!(!store.agent_unread(id, Ts::from_millis(10_000_000)).unwrap());
}
