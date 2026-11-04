//! Tests for the store: open, `apply`, atomicity, WAL readers, runs.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{
    Decision, ExtraEvent, Field, Fields, Intent, IntentChange, NewBox,
};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId, RunId};
use moor_schema::tables::{Boxes, Events, Sessions};
use sea_query::{Asterisk, Expr, ExprTrait as _, Iden, Query};

use crate::store::db::DbExt as _;
use crate::testing::clock::FakeClock;

mod controller_id;
mod events_after;
mod events_of;
mod node_count;
mod open_boxes;
mod poison;
mod repos;
mod sleep_reason;
mod turns;

use super::apply::Applied;
use super::box_node::live_on_node_in;
use super::{NewRun, RunOutcome, Store, StoreError};
use moor_core::name::NodeName;

/// Open a store in a temp dir with a fake clock.
pub(super) fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.join("state").join("moor.db"), clock.clone()).unwrap();
    (store, clock)
}

/// A new-box decision.
fn insert(name: &str) -> Decision {
    Decision::insert(NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    })
}

/// Insert a box and return its id.
pub(super) fn new_box(store: &Store, name: &str) -> BoxId {
    store.apply(insert(name)).unwrap().box_id
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

/// Event kinds of a box in id order.
fn kinds(store: &Store, id: BoxId) -> Vec<String> {
    store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

/// Count rows of a table.
fn count(store: &Store, table: impl Iden + 'static) -> i64 {
    let q = Query::select()
        .expr(Expr::col(Asterisk).count())
        .from(table)
        .take();
    store.reader().unwrap().row(q, |r| r.get(0)).unwrap()
}

#[test]
fn primary_session_row_exists_for_each_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "t");
    let q = Query::select()
        .expr(Expr::col(Asterisk).count())
        .from(Sessions::Table)
        .and_where(Expr::col(Sessions::Box).eq(id.get()))
        .and_where(Expr::col(Sessions::Ordinal).eq(1))
        .take();
    let n: i64 = store.reader().unwrap().row(q, |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn full_lifecycle_rows_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "t");
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Checking);

    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(provider());
    let a = store.apply(d).unwrap();
    assert_eq!(
        (a.lifecycle, a.box_state),
        (BoxLifecycle::SettingUp, Some(BoxState::Starting))
    );

    let handle = BoxHandle::new("h1");
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: handle.clone(),
        });
    store.apply(d).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.session, SessionState::Working);
    let b = row.boxed.unwrap();
    assert_eq!((b.state, b.handle), (BoxState::Ready, Some(handle)));

    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Close { reason: None })
        .session(SessionInput::End)
        .box_input(BoxInput::Released)
        .event(ExtraEvent::Copied {
            from: "a".into(),
            to: "b".into(),
        });
    let a = store.apply(d).unwrap();
    assert_eq!(a.events.len(), 3);
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.boxed.unwrap().state),
        (BoxLifecycle::Closed, BoxState::Deleted)
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
fn new_box_with_illegal_box_input_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let d = insert("t")
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        })
        .set(provider());
    let err = store.apply(d).unwrap_err();
    assert!(matches!(err, StoreError::Illegal(_)), "{err:?}");
    assert_eq!(
        (count(&store, Boxes::Table), count(&store, Events::Table)),
        (0, 0)
    );
}

#[test]
fn illegal_box_step_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "t");
    let before = store.events(id, 100).unwrap().len();
    let set = Fields {
        prompt: Field::Set("g".into()),
        ..provider()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Wake)
        .session(SessionInput::AgentBusy)
        .box_input(BoxInput::Requested)
        .set(set);
    assert!(matches!(
        store.apply(d).unwrap_err(),
        StoreError::Illegal(_)
    ));
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.prompt, row.boxed),
        (BoxLifecycle::Checking, None, None)
    );
    assert_eq!(store.events(id, 100).unwrap().len(), before);
}

#[test]
fn unknown_existing_id_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let err = store
        .apply(
            Decision::new(BoxId::new(42))
                .lifecycle(BoxLifecycleInput::Close { reason: None })
                .session(SessionInput::End),
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");
}

#[test]
fn box_fields_without_a_box_roll_back_the_box_step() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "t");
    let before = store.box_row(id).unwrap().unwrap();
    assert_eq!(
        (before.lifecycle, before.boxed.is_none()),
        (BoxLifecycle::Checking, true)
    );
    let events_before = kinds(&store, id);
    // step_box (Checking -> SettingUp) and write_box_row both succeed; only
    // write_box fails, so the box UPDATE must be rolled back.
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .set(Fields {
            box_workdir: Field::Set("/w".into()),
            ..Fields::default()
        });
    let err = store.apply(d).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    let after = store.box_row(id).unwrap().unwrap();
    assert_eq!(after, before);
    assert_eq!(after.lifecycle, BoxLifecycle::Checking);
    assert_eq!(kinds(&store, id), events_before);
    assert_eq!(
        count(&store, Events::Table),
        i64::try_from(events_before.len()).unwrap()
    );
}

#[test]
fn event_ids_increase_and_ts_follows_clock() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = new_box(&store, "t");
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
    let id = new_box(&store, "t");
    let set = |f| {
        Decision::new(id).set(Fields {
            setup_failed_at: f,
            ..Fields::default()
        })
    };
    store.apply(set(Field::Set(4_242))).unwrap();
    assert_eq!(
        store.box_row(id).unwrap().unwrap().setup_failed_at,
        Some(Ts::from_millis(4_242))
    );
    store.apply(set(Field::Clear)).unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().setup_failed_at, None);
}

#[test]
fn intent_set_clear_and_run_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "t");
    store
        .apply(Decision::new(id).intent(IntentChange::Set(Intent::Provisioning { pid: 7 })))
        .unwrap();
    assert_eq!(
        store.box_row(id).unwrap().unwrap().intent,
        Some(Intent::Provisioning { pid: 7 })
    );
    store
        .apply(Decision::new(id).intent(IntentChange::Clear))
        .unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().intent, None);

    let (run, _claim) = store
        .begin_run(NewRun {
            box_id: id,
            argv: vec!["ls".into()],
            record: None,
            sha: None,
            pid: 9,
        })
        .unwrap();
    assert_eq!(
        store.box_row(id).unwrap().unwrap().intent,
        Some(Intent::Running { run, pid: 9 })
    );
    store
        .finish_run(run, RunOutcome::ended(Some(3), false))
        .unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().intent, None);
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

/// The node the racing Boxes share.
fn node_p() -> NodeName {
    NodeName::parse("p").unwrap()
}

/// Claim the only slot of node `p` for `id`, or queue the box when it is taken.
fn claim_last_slot(store: &Store, id: BoxId) -> Option<Applied> {
    store
        .apply_admitting(id, &node_p(), |(used, _)| {
            let d = Decision::new(id);
            Some(if used == 0 {
                d.lifecycle(BoxLifecycleInput::Admitted)
                    .box_input(BoxInput::Requested)
                    .set(self::provider())
            } else {
                d.lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 })
            })
        })
        .unwrap()
}

#[test]
fn two_boxes_racing_for_the_last_slot_cannot_both_be_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (a, b) = (new_box(&store, "a"), new_box(&store, "b"));
    // A second store on the same file stands in for a second `moor start` process:
    // only SQLite's write lock, not the in-process writer mutex, can separate the two.
    let rival_store = Store::open(
        &dir.path().join("state").join("moor.db"),
        Arc::new(FakeClock::new(Ts::from_millis(1_000_000))),
    )
    .unwrap();
    let node = node_p();
    let (lock_held, rival_may_claim) = std::sync::mpsc::channel::<()>();
    let admitted = std::thread::scope(|s| {
        let rival_store = &rival_store;
        let rival = s.spawn(move || {
            rival_may_claim.recv().unwrap();
            claim_last_slot(rival_store, b)
        });
        let first = store
            .apply_admitting(a, &node, |(used, _)| {
                assert_eq!(used, 0);
                lock_held.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(300));
                Some(
                    Decision::new(a)
                        .lifecycle(BoxLifecycleInput::Admitted)
                        .box_input(BoxInput::Requested)
                        .set(self::provider()),
                )
            })
            .unwrap();
        let second = rival.join().unwrap();
        [first, second]
    });
    let states = [a, b].map(|id| store.box_row(id).unwrap().unwrap().lifecycle);
    assert_eq!(admitted.map(|x| x.is_some()), [true, true]);
    assert_eq!(states, [BoxLifecycle::SettingUp, BoxLifecycle::Pending]);
    assert_eq!(
        live_on_node_in(&store.reader().unwrap(), &node).unwrap().0,
        1
    );
}
