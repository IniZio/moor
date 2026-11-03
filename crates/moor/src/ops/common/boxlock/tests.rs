//! Tests for the per-Box flock guard.

use std::time::Duration;

use moor_core::name::BoxId;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::ErrorKind;

use super::{interrupt, is_interruptible, wait_ended, BoxLock};
use crate::cx::Cancel;
use crate::testing::clock::FakeClock;

const WAIT: Duration = Duration::from_secs(2);

#[test]
fn second_holder_is_refused_as_busy() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(7), "start", WAIT).unwrap();
    let err = BoxLock::acquire(&clock, dir.path(), BoxId::new(7), "close", WAIT).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: start in progress");
}

#[test]
fn wait_is_bounded_by_the_clock() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(7), "start", WAIT).unwrap();
    let from = clock.instant();
    BoxLock::acquire(&clock, dir.path(), BoxId::new(7), "run", WAIT).unwrap_err();
    assert!(clock.instant() - from >= WAIT);
}

#[test]
fn lock_file_lives_under_state_locks() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(42), "start", WAIT).unwrap();
    assert!(dir.path().join("locks/42.box.lock").is_file());
}

#[test]
fn stale_file_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("locks")).unwrap();
    std::fs::write(dir.path().join("locks/3.box.lock"), b"").unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    BoxLock::acquire(&clock, dir.path(), BoxId::new(3), "start", WAIT).unwrap();
}

#[test]
fn stale_file_with_old_op_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("locks")).unwrap();
    std::fs::write(dir.path().join("locks/3.box.lock"), b"close").unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    BoxLock::acquire(&clock, dir.path(), BoxId::new(3), "start", WAIT).unwrap();
}

#[test]
fn empty_holder_file_falls_back_to_generic_text() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(5), "start", WAIT).unwrap();
    std::fs::write(dir.path().join("locks/5.box.lock"), b"").unwrap();
    let err = BoxLock::acquire(&clock, dir.path(), BoxId::new(5), "close", WAIT).unwrap_err();
    assert_eq!(
        err.error.to_string(),
        "box busy: another operation in progress"
    );
}

#[test]
fn drop_releases_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let held = BoxLock::acquire(&clock, dir.path(), BoxId::new(1), "start", WAIT).unwrap();
    drop(held);
    BoxLock::acquire(&clock, dir.path(), BoxId::new(1), "close", WAIT).unwrap();
}

#[test]
fn other_boxes_do_not_contend() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _a = BoxLock::acquire(&clock, dir.path(), BoxId::new(1), "start", WAIT).unwrap();
    BoxLock::acquire(&clock, dir.path(), BoxId::new(2), "start", WAIT).unwrap();
}

#[test]
fn shorter_op_replaces_longer_stale_op() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("locks")).unwrap();
    std::fs::write(dir.path().join("locks/9.box.lock"), b"start").unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(9), "run", WAIT).unwrap();
    let err = BoxLock::acquire(&clock, dir.path(), BoxId::new(9), "close", WAIT).unwrap_err();
    assert_eq!(err.error.to_string(), "box busy: run in progress");
}

#[test]
fn interrupt_stops_the_holder_and_hears_the_hold_end() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let cancel = Cancel::default();
    let held = BoxLock::acquire(&clock, dir.path(), BoxId::new(4), "start", WAIT)
        .unwrap()
        .interruptible(&cancel);
    assert!(is_interruptible(dir.path(), BoxId::new(4)));
    let ended = interrupt(dir.path(), BoxId::new(4)).unwrap();
    assert!(cancel.is_cancelled());
    drop(held);
    assert!(wait_ended(&clock, &ended, Duration::ZERO));
    assert!(!is_interruptible(dir.path(), BoxId::new(4)));
}

#[test]
fn a_plain_hold_is_not_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(6), "run", WAIT).unwrap();
    assert!(interrupt(dir.path(), BoxId::new(6)).is_none());
}

#[test]
fn waiting_on_a_hold_that_never_ends_gives_up_on_the_clock() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let cancel = Cancel::default();
    let _held = BoxLock::acquire(&clock, dir.path(), BoxId::new(8), "start", WAIT)
        .unwrap()
        .interruptible(&cancel);
    let ended = interrupt(dir.path(), BoxId::new(8)).unwrap();
    assert!(!wait_ended(&clock, &ended, Duration::ZERO));
}

#[test]
fn drop_releases_the_lock_while_a_copy_of_its_fd_is_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(Ts::from_millis(0));
    let held = BoxLock::acquire(&clock, dir.path(), BoxId::new(1), "start", WAIT).unwrap();
    // A child forked by any thread keeps such a copy until it execs.
    let _copy = held.file.try_clone().unwrap();
    drop(held);
    BoxLock::acquire(&clock, dir.path(), BoxId::new(1), "retry", Duration::ZERO).unwrap();
}
