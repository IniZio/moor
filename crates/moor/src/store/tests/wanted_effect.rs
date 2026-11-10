//! What moor wants a Box to end as, and the effect in flight, follow the store's rules whichever
//! op writes first: a weaker wanted action never replaces a stronger one, the lifecycle step that
//! fulfils one clears it, and an effect begins only when none is in flight.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, Effect, EffectChange, WantChange, Wanted, WantedKind};
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, RunId};

use super::hands_fence::ready_box;
use super::open;
use crate::store::{Store, StoreError};

fn close(force: bool) -> Wanted {
    Wanted::Close {
        force,
        reason: None,
    }
}

fn suspend() -> Wanted {
    Wanted::Suspend {
        reason: "memory".to_owned(),
    }
}

fn want(store: &Store, id: BoxId, change: WantChange) {
    store.apply(Decision::new(id).want(change)).unwrap();
}

fn wanted(store: &Store, id: BoxId) -> Option<Wanted> {
    store.box_row(id).unwrap().unwrap().wanted
}

/// The effect in flight and its attempt.
fn effect(store: &Store, id: BoxId) -> (Option<Effect>, u32) {
    let row = store.box_row(id).unwrap().unwrap();
    (row.effect, row.effect_attempt)
}

#[test]
fn a_suspend_raised_over_a_close_leaves_the_close() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    want(&store, id, WantChange::Raise(suspend()));
    want(&store, id, WantChange::Raise(Wanted::Sleep));

    assert_eq!(wanted(&store, id), Some(close(false)));
}

#[test]
fn a_close_raised_over_a_suspend_outlives_the_suspend_it_raced() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(suspend()));
    want(&store, id, WantChange::Raise(close(true)));

    store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Suspend {
            reason: "memory".to_owned(),
        }))
        .unwrap();

    assert_eq!(wanted(&store, id), Some(close(true)));
}

#[test]
fn a_forced_close_replaces_one_that_was_not_forced() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    want(&store, id, WantChange::Raise(close(true)));
    want(&store, id, WantChange::Raise(close(false)));

    assert_eq!(wanted(&store, id), Some(close(true)));
}

#[test]
fn done_clears_only_its_own_kind() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    want(&store, id, WantChange::Done(WantedKind::Suspend));
    assert_eq!(wanted(&store, id), Some(close(false)));

    want(&store, id, WantChange::Done(WantedKind::Close));
    assert_eq!(wanted(&store, id), None);
}

#[test]
fn closing_the_box_clears_what_it_wanted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close { reason: None })
                .session(SessionInput::End),
        )
        .unwrap();

    assert_eq!(wanted(&store, id), None);
}

#[test]
fn begin_over_an_effect_in_flight_is_invalid_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let begin = |e| Decision::new(id).effect(EffectChange::Begin(e));
    store.apply(begin(Effect::Setup)).unwrap();

    let got = store.apply(begin(Effect::Launch));

    assert!(matches!(got, Err(StoreError::Invalid(_))), "{got:?}");
    assert_eq!(effect(&store, id), (Some(Effect::Setup), 1));
}

#[test]
fn again_bumps_the_attempt_and_end_clears_the_effect() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let run = Effect::Run { run: RunId::new(7) };
    store
        .apply(Decision::new(id).effect(EffectChange::Begin(run)))
        .unwrap();

    store
        .apply(Decision::new(id).effect(EffectChange::Again))
        .unwrap();
    assert_eq!(effect(&store, id), (Some(run), 2));

    store
        .apply(Decision::new(id).effect(EffectChange::End))
        .unwrap();
    assert_eq!(effect(&store, id), (None, 0));
}

#[test]
fn again_with_no_effect_in_flight_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);

    let got = store.apply(Decision::new(id).effect(EffectChange::Again));

    assert!(matches!(got, Err(StoreError::Invalid(_))), "{got:?}");
}
