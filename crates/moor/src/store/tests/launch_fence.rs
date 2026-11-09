//! A write made from a reading of one agent launch names that launch. Once a newer launch began,
//! or new hands ended it, the write is refused and changes nothing.

use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::{BoxId, LaunchId};

use super::hands_fence::ready_box;
use super::{kinds, open};
use crate::store::{Fence, LaunchEnd, Store, StoreError};

/// A working Box with launch n ended for a relaunch and launch n+1 live: n, then n+1.
fn relaunched(store: &Store) -> (BoxId, LaunchId, LaunchId) {
    let id = ready_box(store);
    let old = store.begin_launch(id, "s-1").unwrap();
    store.end_launch_with(old, None, None).unwrap();
    let new = store.begin_launch(id, "s-1").unwrap();
    (id, old, new)
}

fn stale_for(got: &Result<impl std::fmt::Debug, StoreError>, id: BoxId, launch: LaunchId) -> bool {
    matches!(got, Err(StoreError::Stale { box_id, read }) if *box_id == id && *read == Fence::Launch(launch))
}

fn session_state(store: &Store, id: BoxId) -> SessionState {
    store.box_row(id).unwrap().unwrap().session
}

#[test]
fn a_write_for_launch_n_after_n_plus_one_began_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let (id, old, _) = relaunched(&store);
    let before = kinds(&store, id);

    let got = store.apply(
        Decision::new(id)
            .launch(old)
            .session(SessionInput::AgentQuiet),
    );

    assert!(stale_for(&got, id, old), "{got:?}");
    assert_eq!(session_state(&store, id), SessionState::Working);
    assert_eq!(kinds(&store, id), before, "no event for a dropped write");
}

#[test]
fn a_write_for_the_live_launch_lands() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let (id, _, new) = relaunched(&store);

    store
        .apply(
            Decision::new(id)
                .launch(new)
                .session(SessionInput::AgentQuiet),
        )
        .unwrap();

    assert_eq!(session_state(&store, id), SessionState::Idle);
}

#[test]
fn ending_launch_n_after_n_plus_one_began_is_refused_and_ends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let (id, old, new) = relaunched(&store);
    let end = LaunchEnd {
        session: Some("exited"),
        ..LaunchEnd::default()
    };

    let got = store.end_launch(id, old, &end);

    assert!(stale_for(&got, id, old), "{got:?}");
    assert_eq!(store.live_launch(id).unwrap().map(|l| l.id), Some(new));
    assert_eq!(session_state(&store, id), SessionState::Working);
}

#[test]
fn ending_the_live_launch_with_a_cause_ends_its_session_in_the_same_write() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let (id, _, new) = relaunched(&store);
    let end = LaunchEnd {
        exit: Some(3),
        session: Some("exited"),
        ..LaunchEnd::default()
    };

    store.end_launch(id, new, &end).unwrap();

    assert_eq!(store.live_launch(id).unwrap(), None);
    assert_eq!(session_state(&store, id), SessionState::Ended);
    assert_eq!(
        kinds(&store, id).last().map(String::as_str),
        Some("session_ended")
    );
}

#[test]
fn a_settle_across_new_hands_is_refused_and_the_session_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let launch = store.begin_launch(id, "s-1").unwrap();
    store
        .apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
    store
        .apply(Decision::new(id).hands_input(HandsInc::new(2), HandsInput::Requested))
        .unwrap();

    let got = store.apply(
        Decision::new(id)
            .launch(launch)
            .session(SessionInput::AgentQuiet),
    );

    assert!(stale_for(&got, id, launch), "{got:?}");
    assert_eq!(store.live_launch(id).unwrap(), None);
    assert_ne!(session_state(&store, id), SessionState::Ended);
}
