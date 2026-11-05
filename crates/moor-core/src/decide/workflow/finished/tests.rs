//! Tests for when a workflow run is finished.

use super::{run_finished, RunBoxFacts, QUIET_BEFORE_FINISHED};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

const NOW: i64 = 10_000_000;

fn facts(lifecycle: BoxLifecycle, session: SessionState, idle_for_ms: Option<i64>) -> RunBoxFacts {
    RunBoxFacts {
        lifecycle,
        session,
        idle_since: idle_for_ms.map(|ms| Ts::from_millis(NOW - ms)),
    }
}

fn finished(f: Option<RunBoxFacts>) -> bool {
    run_finished(f.as_ref(), Ts::from_millis(NOW))
}

fn quiet_ms() -> i64 {
    i64::try_from(QUIET_BEFORE_FINISHED.as_millis()).unwrap()
}

#[test]
fn a_gone_or_closed_box_ends_the_run() {
    assert!(finished(None));
    for lifecycle in [BoxLifecycle::Closed, BoxLifecycle::Rejected] {
        assert!(finished(Some(facts(
            lifecycle,
            SessionState::Working,
            None
        ))));
    }
}

#[test]
fn a_working_or_starting_agent_is_not_finished_however_long_it_runs() {
    for session in [SessionState::Working, SessionState::Starting] {
        assert!(!finished(Some(facts(BoxLifecycle::Ready, session, None))));
    }
}

#[test]
fn an_agent_quiet_only_for_a_moment_is_between_turns() {
    let f = |ms| {
        finished(Some(facts(
            BoxLifecycle::Ready,
            SessionState::Idle,
            Some(ms),
        )))
    };
    assert!(!f(0));
    assert!(!f(quiet_ms() - 1));
    assert!(f(quiet_ms()));
}

#[test]
fn an_idle_session_without_a_stamp_is_not_finished() {
    assert!(!finished(Some(facts(
        BoxLifecycle::Ready,
        SessionState::Idle,
        None
    ))));
}

#[test]
fn a_sleeping_box_whose_agent_was_idle_is_finished() {
    let f = facts(BoxLifecycle::Asleep, SessionState::Idle, Some(quiet_ms()));
    assert!(finished(Some(f)));
}

#[test]
fn an_exited_agent_ends_the_run_but_a_box_still_setting_up_does_not() {
    assert!(finished(Some(facts(
        BoxLifecycle::Ready,
        SessionState::Ended,
        None
    ))));
    assert!(!finished(Some(facts(
        BoxLifecycle::Pending,
        SessionState::Ended,
        None
    ))));
    assert!(!finished(Some(facts(
        BoxLifecycle::SetupFailed,
        SessionState::Ended,
        None
    ))));
}
