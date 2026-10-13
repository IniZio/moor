//! Tests for the auto-sleep decider.

use std::time::Duration;

use super::{sleep_due, SleepFacts, SleepVerdict};
use crate::domain::task::TaskState;
use crate::seam::clock::Ts;

const AFTER: Duration = Duration::from_secs(600);

fn facts(state: TaskState, idle_since: Option<i64>, now: i64) -> SleepFacts {
    SleepFacts {
        state,
        idle_since: idle_since.map(Ts::from_millis),
        now: Ts::from_millis(now),
        after: AFTER,
    }
}

#[test]
fn idle_exactly_at_threshold_is_due() {
    let f = facts(TaskState::Idle, Some(1_000), 1_000 + 600_000);
    assert_eq!(sleep_due(&f), SleepVerdict::Due);
}

#[test]
fn idle_past_threshold_is_due() {
    let f = facts(TaskState::Idle, Some(1_000), 1_000 + 700_000);
    assert_eq!(sleep_due(&f), SleepVerdict::Due);
}

#[test]
fn idle_one_second_before_threshold_is_not_yet() {
    let f = facts(TaskState::Idle, Some(1_000), 1_000 + 599_000);
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn working_far_past_threshold_is_not_idle() {
    let f = facts(TaskState::Working, Some(0), 10_000_000);
    assert_eq!(sleep_due(&f), SleepVerdict::NotIdle);
}

#[test]
fn every_other_state_is_not_idle() {
    for state in TaskState::ALL {
        if state == TaskState::Idle {
            continue;
        }
        let f = facts(state, Some(0), 10_000_000);
        assert_eq!(sleep_due(&f), SleepVerdict::NotIdle, "{state:?}");
    }
}

#[test]
fn idle_without_idle_since_is_not_yet() {
    let f = facts(TaskState::Idle, None, 10_000_000);
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn clock_going_backwards_is_not_yet() {
    let f = facts(TaskState::Idle, Some(10_000_000), 1_000);
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}

#[test]
fn extreme_timestamps_do_not_overflow() {
    let f = facts(TaskState::Idle, Some(i64::MIN), i64::MAX);
    assert_eq!(sleep_due(&f), SleepVerdict::Due);
    let f = facts(TaskState::Idle, Some(i64::MAX), i64::MIN);
    assert_eq!(sleep_due(&f), SleepVerdict::NotYet);
}
