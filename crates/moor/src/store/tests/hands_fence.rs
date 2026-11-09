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
use crate::store::{Fence, Store, StoreError, SuspendCheck};

const OLD: HandsInc = HandsInc::FIRST;
const NEW: HandsInc = HandsInc::new(2);

/// A working Box on its first hands, Ready on `h1`.
pub(super) fn ready_box(store: &Store) -> BoxId {
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

impl Store {
    /// Gives Box `id` second hands, Starting, as a re-acquire after losing the first would.
    pub(crate) fn replace_hands(&self, id: BoxId) {
        self.writer()
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
}

impl Store {
    /// Every hands row of Box `id`, oldest first: `inc`, state and whether it ended.
    pub(crate) fn hands_history(&self, id: BoxId) -> Vec<(i64, String, bool)> {
        self.writer()
            .rows(
                Query::select()
                    .columns([BoxHands::Inc, BoxHands::State, BoxHands::EndedAt])
                    .from(BoxHands::Table)
                    .and_where(Expr::col(BoxHands::Box).eq(id.get()))
                    .order_by(BoxHands::Inc, sea_query::Order::Asc),
                |r| {
                    let ended: Option<i64> = r.get(2)?;
                    Ok::<_, crate::store::db::Error>((r.get(0)?, r.get(1)?, ended.is_some()))
                },
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
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
    store.replace_hands(id);
    let before = kinds(&store, id);

    let err = store.apply(gone).unwrap_err();

    assert!(
        matches!(err, StoreError::Stale { box_id, read } if box_id == id && read == Fence::Hands(OLD)),
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
    store.replace_hands(id);

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
    store.replace_hands(id);
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

#[test]
fn a_request_after_lost_hands_adds_new_hands_and_keeps_the_old_as_ended() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    store
        .apply(Decision::new(id).hands_input(OLD, HandsInput::DriverMissing))
        .unwrap();

    store
        .apply(Decision::new(id).hands_input(NEW, HandsInput::Requested))
        .unwrap();

    assert_eq!(
        store.hands_history(id),
        [
            (1, "gone".to_owned(), true),
            (2, "starting".to_owned(), false)
        ]
    );
    let hands = store.box_row(id).unwrap().unwrap().hands.unwrap();
    assert_eq!(hands.provider.as_str(), "p", "new hands keep the provider");
}

#[test]
fn ended_hands_are_never_acquired_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    store
        .apply(Decision::new(id).hands_input(OLD, HandsInput::DriverMissing))
        .unwrap();

    let again = store.apply(Decision::new(id).hands_input(OLD, HandsInput::Requested));

    assert!(matches!(again, Err(StoreError::Illegal(_))), "{again:?}");
    assert_eq!(store.hands_history(id), [(1, "gone".to_owned(), true)]);
}

#[test]
fn a_late_verdict_on_lost_hands_is_refused_once_new_hands_exist() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    store
        .apply(Decision::new(id).hands_input(OLD, HandsInput::DriverMissing))
        .unwrap();
    store
        .apply(Decision::new(id).hands_input(NEW, HandsInput::Requested))
        .unwrap();

    let late = store.apply(Decision::new(id).hands_input(OLD, HandsInput::Released));

    assert!(
        matches!(late, Err(StoreError::Stale { read, .. }) if read == Fence::Hands(OLD)),
        "{late:?}"
    );
    assert_eq!(
        store.hands_history(id),
        [
            (1, "gone".to_owned(), true),
            (2, "starting".to_owned(), false)
        ]
    );
}

#[test]
fn hands_that_are_still_acquiring_get_no_second_row() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);

    let skip = store.apply(Decision::new(id).hands_input(NEW, HandsInput::Requested));

    assert!(matches!(skip, Err(StoreError::Stale { .. })), "{skip:?}");
    assert_eq!(store.hands_history(id), [(1, "ready".to_owned(), false)]);
}
