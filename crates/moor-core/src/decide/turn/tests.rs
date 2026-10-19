//! Tests for the turn decider.

use super::{turn_done, HookSignal, TurnFacts};
use crate::domain::event::TurnOutcome;
use crate::seam::clock::Ts;

fn rec(signal: HookSignal, at: i64) -> (HookSignal, Ts) {
    (signal, Ts::from_millis(at))
}

fn facts(hook: Option<(HookSignal, Ts)>, exited: bool) -> TurnFacts {
    TurnFacts {
        steer: Some((7, Ts::from_millis(100))),
        hook,
        pane_exited: exited,
    }
}

#[test]
fn idle_after_steer_is_idle() {
    let h = rec(HookSignal::Idle, 101);
    assert_eq!(
        turn_done(&facts(Some(h), false)),
        Some((7, TurnOutcome::Idle))
    );
}

#[test]
fn blocked_after_steer_needs_input() {
    let h = rec(HookSignal::Blocked, 150);
    assert_eq!(
        turn_done(&facts(Some(h), false)),
        Some((7, TurnOutcome::NeedsInput))
    );
}

#[test]
fn hook_at_or_before_steer_is_none() {
    for at in [100, 99] {
        let h = rec(HookSignal::Idle, at);
        assert_eq!(turn_done(&facts(Some(h), false)), None);
    }
}

#[test]
fn no_unanswered_steer_is_none() {
    let h = rec(HookSignal::Idle, 500);
    let f = TurnFacts {
        steer: None,
        hook: Some(h),
        pane_exited: true,
    };
    assert_eq!(turn_done(&f), None);
}

#[test]
fn exited_pane_wins_over_hook() {
    let h = rec(HookSignal::Idle, 500);
    assert_eq!(
        turn_done(&facts(Some(h), true)),
        Some((7, TurnOutcome::Exited))
    );
    assert_eq!(
        turn_done(&facts(None, true)),
        Some((7, TurnOutcome::Exited))
    );
}

#[test]
fn busy_or_starting_is_none() {
    {
        let h = rec(HookSignal::Other, 500);
        assert_eq!(turn_done(&facts(Some(h), false)), None);
    }
}

#[test]
fn no_hook_is_none() {
    assert_eq!(turn_done(&facts(None, false)), None);
}

#[test]
fn outcome_names() {
    assert_eq!(TurnOutcome::NeedsInput.as_str(), "needs_input");
    assert_eq!(
        serde_json::to_string(&TurnOutcome::Exited).unwrap(),
        "\"exited\""
    );
    assert_eq!(TurnOutcome::Idle.as_str(), "idle");
}
