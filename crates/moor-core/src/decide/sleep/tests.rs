//! Tests for the auto-sleep decider.

use std::time::Duration;

use super::{sleep_due, SleepFacts, SleepVerdict};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

const AFTER: Duration = Duration::from_secs(600);
const BLOCKED_MS: i64 = 300_000;
const MAX_AWAKE_MS: i64 = 7_200_000;

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
        after_blocked: Duration::from_secs(300),
        max_awake: Duration::from_secs(7200),
        blocked: false,
        awake_since: None,
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

fn limited(session: SessionState, blocked: bool, awake_since: Option<i64>, now: i64) -> SleepFacts {
    SleepFacts {
        blocked,
        awake_since: awake_since.map(Ts::from_millis),
        ..facts(BoxLifecycle::Ready, session, Some(0), now)
    }
}

#[test]
fn a_box_waiting_on_a_prompt_sleeps_after_the_blocked_timer() {
    let at = |now| sleep_due(&limited(SessionState::Idle, true, None, now));
    assert_eq!(at(BLOCKED_MS - 1), SleepVerdict::NotYet);
    assert_eq!(at(BLOCKED_MS), SleepVerdict::Due);
}

#[test]
fn an_idle_box_not_waiting_keeps_the_idle_timer() {
    let f = limited(SessionState::Idle, false, None, BLOCKED_MS);
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn any_box_awake_past_the_cap_is_due_whatever_its_session() {
    for session in [SessionState::Working, SessionState::Starting] {
        let at = |now| sleep_due(&limited(session, false, Some(0), now));
        assert_eq!(at(MAX_AWAKE_MS - 1), SleepVerdict::NotIdle, "{session:?}");
        assert_eq!(at(MAX_AWAKE_MS), SleepVerdict::Due, "{session:?}");
    }
}

#[test]
fn the_awake_cap_needs_a_ready_box_and_a_known_wake() {
    let asleep = SleepFacts {
        awake_since: Some(Ts::from_millis(0)),
        ..facts(
            BoxLifecycle::Asleep,
            SessionState::Working,
            None,
            MAX_AWAKE_MS,
        )
    };
    assert_eq!(sleep_due(&asleep), SleepVerdict::NotIdle);
    let unknown = limited(SessionState::Working, false, None, MAX_AWAKE_MS);
    assert_eq!(sleep_due(&unknown), SleepVerdict::NotIdle);
}
