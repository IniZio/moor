//! Tests for when a workflow run is finished.

use super::{run_verdict, RunBoxFacts, RunVerdict, QUIET_BEFORE_FINISHED};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::seam::clock::Ts;

const NOW: i64 = 10_000_000;

fn facts(lifecycle: BoxLifecycle, session: SessionState, idle_for_ms: Option<i64>) -> RunBoxFacts {
    RunBoxFacts {
        lifecycle,
        session,
        idle_since: idle_for_ms.map(|ms| Ts::from_millis(NOW - ms)),
        launch_failed: false,
    }
}

fn finished(f: Option<RunBoxFacts>) -> bool {
    run_verdict(f.as_ref(), Ts::from_millis(NOW)) == RunVerdict::Finished
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

fn verdict(lifecycle: BoxLifecycle, session: SessionState, launch_failed: bool) -> RunVerdict {
    let f = RunBoxFacts {
        launch_failed,
        ..facts(lifecycle, session, None)
    };
    run_verdict(Some(&f), Ts::from_millis(NOW))
}

#[test]
fn every_lifecycle_has_a_verdict_for_a_working_agent() {
    use BoxLifecycle as L;
    use RunVerdict::{Broken, Finished, Running};
    let table = [
        (L::Checking, Running),
        (L::Rejected, Finished),
        (L::Pending, Running),
        (L::SettingUp, Running),
        (L::SetupFailed, Broken),
        (L::Ready, Running),
        (L::Asleep, Running),
        (L::Closed, Finished),
    ];
    assert_eq!(table.len(), L::ALL.len());
    for (lifecycle, want) in table {
        assert_eq!(
            verdict(lifecycle, SessionState::Working, false),
            want,
            "{lifecycle:?}"
        );
    }
}

#[test]
fn a_failed_setup_is_broken_whatever_the_session_says() {
    for session in SessionState::ALL {
        assert_eq!(
            verdict(BoxLifecycle::SetupFailed, session, false),
            RunVerdict::Broken,
            "{session:?}"
        );
    }
}

#[test]
fn a_failed_launch_is_broken_on_a_box_that_is_up_but_not_when_it_is_closed() {
    for lifecycle in [BoxLifecycle::Ready, BoxLifecycle::Asleep] {
        for session in [
            SessionState::Ended,
            SessionState::Working,
            SessionState::Idle,
        ] {
            assert_eq!(verdict(lifecycle, session, true), RunVerdict::Broken);
        }
    }
    assert_eq!(
        verdict(BoxLifecycle::Closed, SessionState::Ended, true),
        RunVerdict::Finished
    );
}
