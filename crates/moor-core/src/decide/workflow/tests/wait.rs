//! Tests for runs that wait for an answer: working time and the week without one.

use super::{daily, finished, started, step, ts, waiting, workflow_expired, DAY, DAY0, HOUR};
use crate::decide::workflow::{waited_millis, workflow_unanswered};
use crate::seam::clock::Ts;
use crate::user::UserId;
use crate::workflow::{Answer, HistoryEntry, RunId, StepId, TimeoutMinutes};

const MIN: i64 = 60_000;

fn answered(step: &str, at: i64) -> (Ts, HistoryEntry) {
    (
        ts(at),
        HistoryEntry::Answered {
            step: StepId::parse(step).unwrap(),
            by: UserId::new(1),
            answer: Answer::Yes,
            note: None,
        },
    )
}

fn entered(step: &str, at: i64) -> (Ts, HistoryEntry) {
    (
        ts(at),
        HistoryEntry::Entered {
            step: StepId::parse(step).unwrap(),
            try_n: 1,
        },
    )
}

#[test]
fn waiting_time_is_each_span_from_entering_an_approve_step_to_its_answer() {
    let lines = [
        entered("a", 0),
        entered("ask", 0),
        answered("ask", HOUR),
        entered("ask2", 2 * HOUR),
        answered("ask2", 10 * HOUR),
    ];
    assert_eq!(waited_millis(&lines, ts(0)), HOUR + 8 * HOUR);
    assert_eq!(waited_millis(&lines[..2], ts(0)), 0);
}

#[test]
fn a_wait_before_the_clock_started_is_not_part_of_it() {
    // A run that waited at its first step, 0 to 5 hours, and whose agent began at 4 hours.
    let lines = [entered("ask", 0), answered("ask", 5 * HOUR)];
    assert_eq!(waited_millis(&lines, ts(0)), 5 * HOUR);
    assert_eq!(waited_millis(&lines, ts(4 * HOUR)), HOUR);
    assert_eq!(waited_millis(&lines, ts(6 * HOUR)), 0);
}

#[test]
fn the_start_limit_does_not_count_the_wait_at_a_first_approve_step() {
    use crate::decide::workflow::{workflow_start_overdue, START_LIMIT};
    let hour = i64::try_from(START_LIMIT.as_millis()).unwrap();
    let open = [started("a/b", DAY0, 7)];
    let none = |_| None;
    let waited = |_: RunId, _: Ts| 5 * DAY;
    let now = ts(DAY0 + 5 * DAY + hour - 1);
    assert!(workflow_start_overdue(&open, (&none, &waited), now).is_empty());
    let now = ts(DAY0 + 5 * DAY + hour);
    assert_eq!(
        workflow_start_overdue(&open, (&none, &waited), now).len(),
        1
    );
}

#[test]
fn a_wait_not_answered_yet_is_not_counted() {
    let lines = [entered("a", 0), entered("ask", HOUR)];
    assert_eq!(waited_millis(&lines, ts(0)), 0);
}

#[test]
fn the_timeout_clock_stops_while_a_run_waits_for_an_answer() {
    let mut w = daily();
    w.timeout = TimeoutMinutes::new(60).unwrap();
    let open = [started("a/b", DAY0, 7)];
    let agent = |_| Some(ts(DAY0));
    // 5 hours on the wall clock, 4.5 of them spent waiting, so 30 working minutes.
    let waited = |_: RunId, _: Ts| 270 * MIN;
    let at = |ms: i64| workflow_expired(&w, &open, &agent, &waited, ts(DAY0 + ms));
    assert!(at(300 * MIN).is_empty());
    assert!(at(329 * MIN).is_empty());
    assert_eq!(at(330 * MIN).len(), 1);
    // Without the waits the same run would be long expired.
    assert_eq!(
        workflow_expired(&w, &open, &agent, &|_, _| 0, ts(DAY0 + 300 * MIN)).len(),
        1
    );
}

#[test]
fn a_waiting_run_never_expires_on_the_timeout() {
    let w = daily();
    let open = [waiting("a/b", DAY0)];
    let agent = |_| Some(ts(DAY0));
    assert!(workflow_expired(&w, &open, &agent, &|_, _| 0, ts(DAY0 + 30 * DAY)).is_empty());
}

#[test]
fn a_wait_ends_after_seven_days_without_an_answer() {
    let open = [
        waiting("a/b", DAY0),
        started("c/d", DAY0, 8),
        finished("e/f", DAY0),
    ];
    assert!(workflow_unanswered(&open, ts(DAY0 + 7 * DAY - 1)).is_empty());
    let late = workflow_unanswered(&open, ts(DAY0 + 7 * DAY));
    assert_eq!(late.len(), 1);
    let first = late.first().unwrap();
    assert_eq!(first.run, open[0].id);
    assert_eq!(first.at, step());
}
