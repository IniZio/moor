use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::PausePublish;
use moor_core::{BoxHandle, BoxId};
use moor_schema::tables::BoxHands;
use sea_query::{Expr, ExprTrait as _, Query};

use super::{kinds, new_box, open, provider};
use crate::store::db::DbExt as _;
use crate::store::{Store, StoreError, SuspendCheck};

const OLD: HandsInc = HandsInc::FIRST;
const NEW: HandsInc = HandsInc::new(2);

/// A working Box on its first hands, Ready on `h1`.
fn ready_box(store: &Store) -> BoxId {
    let id = new_box(store, "t");
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Admitted)
                .hands_input(OLD, HandsInput::Requested)
                .hands(OLD, provider()),
        )
        .unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: SetupOutcome::Ok,
                })
                .session(SessionInput::Started)
                .hands_input(
                    OLD,
                    HandsInput::Acquired {
                        handle: BoxHandle::new("h1"),
                    },
                ),
        )
        .unwrap();
    id
}

/// Gives Box `id` second hands, Starting, as a re-acquire after losing the first would.
fn replace_hands(store: &Store, id: BoxId) {
    store
        .writer()
        .run(
            Query::insert()
                .into_table(BoxHands::Table)
                .columns([
                    BoxHands::Box,
                    BoxHands::Inc,
                    BoxHands::Provider,
                    BoxHands::State,
                    BoxHands::StateAt,
                    BoxHands::CreatedAt,
                ])
                .values_panic([
                    id.get().into(),
                    NEW.get().into(),
                    "p".into(),
                    HandsState::Starting.as_str().into(),
                    1_i64.into(),
                    1_i64.into(),
                ]),
        )
        .unwrap();
}

/// The stored state of Box `id`'s hands `inc`.
fn state_of(store: &Store, id: BoxId, inc: HandsInc) -> String {
    store
        .writer()
        .row(
            Query::select()
                .column(BoxHands::State)
                .from(BoxHands::Table)
                .and_where(Expr::col(BoxHands::Box).eq(id.get()))
                .and_where(Expr::col(BoxHands::Inc).eq(inc.get())),
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_gone_verdict_on_replaced_hands_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let fact = BoxFact {
        box_id: id,
        inc: OLD,
        state: HandsState::Ready,
        handle: Some(BoxHandle::new("h1")),
    };
    let gone = reconcile(&[fact], &[]).pop().expect("a Gone verdict");
    replace_hands(&store, id);
    let before = kinds(&store, id);

    let err = store.apply(gone).unwrap_err();

    assert!(
        matches!(err, StoreError::Stale { box_id, read } if box_id == id && read == OLD),
        "{err:?}"
    );
    assert_eq!(state_of(&store, id, NEW), "starting");
    assert_eq!(state_of(&store, id, OLD), "ready");
    assert_eq!(kinds(&store, id), before, "no event for a dropped write");
}

#[test]
fn a_write_on_the_current_hands_lands() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    replace_hands(&store, id);

    store
        .apply(Decision::new(id).hands_input(
            NEW,
            HandsInput::Acquired {
                handle: BoxHandle::new("h2"),
            },
        ))
        .unwrap();

    let hands = store.box_row(id).unwrap().unwrap().hands.unwrap();
    assert_eq!((hands.inc, hands.state), (NEW, HandsState::Ready));
    assert_eq!(hands.handle, Some(BoxHandle::new("h2")));
}

#[test]
fn a_gone_verdict_on_the_first_hands_lands_while_they_are_current() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);

    store
        .apply(Decision::new(id).hands_input(OLD, HandsInput::DriverMissing))
        .unwrap();

    assert_eq!(state_of(&store, id, OLD), "gone");
}

#[test]
fn pressure_writes_for_replaced_hands_are_refused_and_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    replace_hands(&store, id);
    let clean = SuspendCheck {
        ahead: 0,
        dirty: 0,
        at: 5,
    };

    let check = store.set_suspend_check((id, OLD), Some(clean));
    let publish = store.set_pause_publish((id, OLD), PausePublish::Allowed);

    assert!(matches!(check, Err(StoreError::Stale { .. })), "{check:?}");
    assert!(
        matches!(publish, Err(StoreError::Stale { .. })),
        "{publish:?}"
    );
    assert_eq!(store.suspend_check(id).unwrap(), None);
    assert_eq!(store.pause_publish(id).unwrap(), PausePublish::Denied);

    store.set_suspend_check((id, NEW), Some(clean)).unwrap();
    store
        .set_pause_publish((id, NEW), PausePublish::Allowed)
        .unwrap();
    assert_eq!(store.suspend_check(id).unwrap(), Some(clean));
    assert_eq!(store.pause_publish(id).unwrap(), PausePublish::Allowed);
}
