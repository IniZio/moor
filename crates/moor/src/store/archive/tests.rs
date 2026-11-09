//! The Session archive pointer moves only forward, and pays the archive due when it moves.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::event::TurnOutcome;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use crate::store::archive::ArchivePointer;
use crate::store::Store;
use crate::testing::clock::FakeClock;

/// A ready Box with a live agent, and its Session's id.
fn fx(dir: &std::path::Path) -> (Store, Arc<FakeClock>, BoxId, i64) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.join("moor.db"), clock.clone());
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(
            HandsInc::FIRST,
            HandsFields {
                provider: Field::Set(ProviderId::new("p")),
                ..HandsFields::default()
            },
        );
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(
            HandsInc::FIRST,
            HandsInput::Acquired {
                handle: BoxHandle::new("h"),
            },
        );
    store.apply(d).unwrap();
    store.begin_session(id, "s-1").unwrap();
    let session = store.live_session(id).unwrap().unwrap().session.unwrap();
    (store, clock, id, session)
}

fn at(key: &str, inc: i64, ms: i64) -> ArchivePointer {
    ArchivePointer {
        key: key.to_owned(),
        inc,
        at: Ts::from_millis(ms),
    }
}

/// Ends a steered turn, which makes the Box's archive due now.
fn end_turn(store: &Store, id: BoxId, steer: i64) {
    assert!(store
        .record_turn_done(id, steer, TurnOutcome::Idle)
        .unwrap());
}

#[test]
fn a_late_save_from_older_hands_does_not_move_the_pointer_back() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock, id, session) = fx(dir.path());
    assert!(store
        .advance_archive(id, session, &at("new", 2, 100))
        .unwrap());
    assert!(!store
        .advance_archive(id, session, &at("late", 1, 500))
        .unwrap());
    assert_eq!(
        store.archive_pointer(session).unwrap(),
        Some(at("new", 2, 100))
    );
}

#[test]
fn the_same_hands_move_the_pointer_only_to_a_later_archive() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock, id, session) = fx(dir.path());
    assert!(store
        .advance_archive(id, session, &at("b", 1, 200))
        .unwrap());
    assert!(!store
        .advance_archive(id, session, &at("a", 1, 100))
        .unwrap());
    assert!(store
        .advance_archive(id, session, &at("c", 1, 300))
        .unwrap());
    assert!(store.advance_archive(id, session, &at("d", 2, 50)).unwrap());
    assert_eq!(
        store.archive_pointer(session).unwrap(),
        Some(at("d", 2, 50))
    );
}

#[test]
fn a_moved_pointer_pays_the_archive_due_by_then_and_a_refused_one_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, id, session) = fx(dir.path());
    assert_eq!(store.archive_due(id).unwrap(), None);
    store
        .advance_archive(id, session, &at("new", 2, 0))
        .unwrap();
    end_turn(&store, id, 1);
    let due = clock.now();
    assert_eq!(store.archive_due(id).unwrap(), Some(due));
    store
        .advance_archive(id, session, &at("late", 1, 9_000))
        .unwrap();
    assert_eq!(store.archive_due(id).unwrap(), Some(due), "a refused save");
    let before = Ts::from_millis(due.as_millis() - 1);
    store
        .advance_archive(id, session, &at("x", 2, before.as_millis()))
        .unwrap();
    assert_eq!(
        store.archive_due(id).unwrap(),
        Some(due),
        "taken before the turn ended"
    );
    store
        .advance_archive(id, session, &at("y", 2, due.as_millis()))
        .unwrap();
    assert_eq!(store.archive_due(id).unwrap(), None);
}

#[test]
fn a_reschedule_keeps_a_due_marked_after_the_save_began() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, id, _session) = fx(dir.path());
    let seen = clock.now();
    clock.sleep(Duration::from_secs(1));
    end_turn(&store, id, 1);
    let later = clock.now();
    store.reschedule_archive(id, seen, None).unwrap();
    assert_eq!(store.archive_due(id).unwrap(), Some(later));
    let retry = later.saturating_add(Duration::from_mins(1));
    store.reschedule_archive(id, later, Some(retry)).unwrap();
    assert_eq!(store.archive_due(id).unwrap(), Some(retry));
}
