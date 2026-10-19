//! Tests for the auto-sleep decider.

use std::time::Duration;

use super::{sleep_due, SleepFacts, SleepVerdict};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

const AFTER: Duration = Duration::from_secs(600);

fn facts(
    lifecycle: BoxLifecycle,
    session: SessionState,
    idle_since: Option<i64>,
    now: i64,
) -> SleepFacts {
    SleepFacts {
        lifecycle,
        session,
        idle_since: idle_since.map(Ts::from_millis),
        now: Ts::from_millis(now),
        after: AFTER,
    }
}

#[test]
fn idle_exactly_at_threshold_is_due() {
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        Some(1_000),
        1_000 + 600_000,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::Due);
}

#[test]
fn idle_past_threshold_is_due() {
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        Some(1_000),
        1_000 + 700_000,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::Due);
}

#[test]
fn idle_one_second_before_threshold_is_not_yet() {
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        Some(1_000),
        1_000 + 599_000,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn working_far_past_threshold_is_not_idle() {
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Working,
        Some(0),
        10_000_000,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::NotIdle);
}

#[test]
fn every_other_pair_is_not_idle() {
    for lifecycle in BoxLifecycle::ALL {
        for session in SessionState::ALL {
            let quiet_ready = lifecycle == BoxLifecycle::Ready && session.is_quiet();
            if quiet_ready {
                continue;
            }
            let f = facts(lifecycle, session, Some(0), 10_000_000);
            assert_eq!(
                sleep_due(&f),
                SleepVerdict::NotIdle,
                "{lifecycle:?} {session:?}"
            );
        }
    }
}

#[test]
fn idle_without_idle_since_is_not_yet() {
    let f = facts(BoxLifecycle::Ready, SessionState::Idle, None, 10_000_000);
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn clock_going_backwards_is_not_yet() {
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        Some(10_000_000),
        1_000,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn extreme_timestamps_do_not_overflow() {
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        Some(i64::MIN),
        i64::MAX,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::Due);
    let f = facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        Some(i64::MAX),
        i64::MIN,
    );
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}
