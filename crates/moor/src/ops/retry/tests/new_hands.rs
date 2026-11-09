//! A retry that forks a kept layer moves the Box onto new hands, so nothing late about the machine
//! it left lands on the fork.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::layers::LayerKey;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::{BoxHandle, BoxId};

use super::{req, row, run, saved_layer, started, world, World, TWO_LINES};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::store::StoreError;
use crate::testing::driver::FakeDriver;

/// A Box whose setup failed its second line, with its first layer kept.
fn failed_with_a_layer() -> (World, BoxId) {
    failed_on(FakeDriver::new())
}

/// A Box on `driver` whose setup failed its second line, with its first layer kept.
fn failed_on(driver: FakeDriver) -> (World, BoxId) {
    let driver = driver
        .with_checkpoints()
        .exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let w = world(TWO_LINES, driver);
    let id = started(&w);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    saved_layer(&w, id, 1);
    (w, id)
}

/// Runs the retry in a process that dies where `w`'s driver panics.
fn retry_crashes(w: &World) {
    let crashed = catch_unwind(AssertUnwindSafe(|| drop(run(&w.cx, &req()))));
    assert!(crashed.is_err(), "the process died");
}

/// The next process: settles the cut-off start of `id` and resumes it.
fn restart(w: &World, id: BoxId) {
    let store = w.cx.store().unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [id]);
    resume_setup(&w.cx, store, id).unwrap();
}

fn releases(w: &World) -> Vec<String> {
    let calls = w.driver.calls().into_iter();
    calls.filter(|c| c.starts_with("release ")).collect()
}

fn history(w: &World, id: BoxId) -> Vec<(i64, String, bool)> {
    w.cx.store().unwrap().hands_history(id)
}

#[test]
fn a_retry_fork_gets_new_hands_and_a_late_write_for_the_old_ones_is_refused() {
    let (w, id) = failed_with_a_layer();

    run(&w.cx, &req()).unwrap();

    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(w.driver.calls().iter().any(|c| c.starts_with("fork")));
    let hands = after.hands.unwrap();
    assert_eq!(hands.inc, HandsInc::new(2));
    assert_ne!(hands.handle, Some(BoxHandle::new("box-1")), "on the fork");
    let store = w.cx.store().unwrap();
    assert_eq!(
        store.hands_history(id),
        [
            (1, "deleted".to_owned(), true),
            (2, "ready".to_owned(), false)
        ],
        "the machine left is released"
    );
    assert_eq!(releases(&w), ["release box-1"]);
    let late = store.apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::Sleep));
    assert!(matches!(late, Err(StoreError::Stale { .. })), "{late:?}");
    assert_eq!(row(&w, id).hands.unwrap().handle, hands.handle);
}

/// The layer saves the retry of `id` in `w` could not record, after a fork or a take-over.
fn saves_refused(w: &World, id: BoxId) -> Vec<String> {
    let failed = w.cx.take_warnings().into_iter();
    let failed = failed.filter(|warn| warn.code == "checkpoint_failed");
    let failed: Vec<String> = failed.map(|warn| warn.message).collect();
    let store = w.cx.store().unwrap();
    let home = row(w, id).hands.unwrap().provider;
    let key = LayerKey::restore("f".repeat(64)).unwrap();
    let late = store.pin_checkpoint((id, Some(HandsInc::FIRST)), &home, &key, 2);
    assert!(matches!(late, Err(StoreError::Stale { .. })), "{late:?}");
    store
        .pin_checkpoint((id, Some(HandsInc::new(2))), &home, &key, 2)
        .unwrap();
    failed
}

#[test]
fn the_fork_saves_its_layers_as_the_new_hands_and_one_saved_by_the_old_is_refused() {
    let (w, id) = failed_with_a_layer();
    drop(w.cx.take_warnings());

    run(&w.cx, &req()).unwrap();

    assert_eq!(saves_refused(&w, id), Vec::<String>::new());
}

#[test]
fn a_fork_taken_over_saves_its_layers_as_the_new_hands() {
    let (w, id) = failed_on(FakeDriver::new().cut_acquires_go_on());
    drop(w.cx.take_warnings());
    w.driver.during_next_fork(|| {});
    w.driver.fork_fails_next(DriverError::Transport {
        stderr_tail: "the stream ended: unexpected end of file".to_owned(),
    });

    run(&w.cx, &req()).unwrap();

    assert!(w.driver.calls().iter().any(|c| c == "adopt"));
    assert_eq!(saves_refused(&w, id), Vec::<String>::new());
}

#[test]
fn a_crash_after_the_move_leaves_the_old_machine_to_the_restart_which_releases_it_once() {
    let (w, id) = failed_with_a_layer();
    w.driver
        .on_release(|| resume_unwind(Box::new("process died before the release")));
    retry_crashes(&w);
    w.driver.on_release(|| {});
    assert_eq!(
        history(&w, id),
        [(1, "gone".to_owned(), true), (2, "ready".to_owned(), false)]
    );
    assert!(releases(&w).is_empty());

    restart(&w, id);

    assert_eq!(releases(&w), ["release box-1"]);
    assert_eq!(
        history(&w, id),
        [
            (1, "deleted".to_owned(), true),
            (2, "ready".to_owned(), false)
        ]
    );
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Ready);
}

#[test]
fn a_crash_before_the_move_keeps_the_old_hands_and_the_restart_releases_the_fork() {
    let (w, id) = failed_with_a_layer();
    w.driver
        .after_next_fork(|| resume_unwind(Box::new("process died before it recorded the fork")));
    retry_crashes(&w);
    assert_eq!(history(&w, id), [(1, "ready".to_owned(), false)]);

    restart(&w, id);

    assert_eq!(releases(&w), ["release box-2"]);
    assert_eq!(history(&w, id), [(1, "ready".to_owned(), false)]);
    let after = row(&w, id);
    assert_eq!(after.hands.unwrap().handle, Some(BoxHandle::new("box-1")));
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
}
