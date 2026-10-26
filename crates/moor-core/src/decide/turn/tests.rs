//! Tests for the turn decider.

use super::{turn_done, HookFacts, HookSignal, SteerMark, TurnFacts};
use crate::domain::event::TurnOutcome;
use crate::seam::clock::Ts;

fn prompt(hook_before: Option<i64>) -> SteerMark {
    SteerMark {
        id: 7,
        hook_before: hook_before.map(Ts::from_millis),
        new_turn: true,
    }
}

fn choice(hook_before: i64) -> SteerMark {
    SteerMark {
        id: 7,
        hook_before: Some(Ts::from_millis(hook_before)),
        new_turn: false,
    }
}

fn rec(signal: HookSignal, at: i64, turn: Option<i64>) -> HookFacts {
    HookFacts {
        signal,
        at: Ts::from_millis(at),
        turn: turn.map(Ts::from_millis),
    }
}

fn decide(steer: SteerMark, hook: Option<HookFacts>, exited: bool) -> Option<(i64, TurnOutcome)> {
    turn_done(&TurnFacts {
        steer: Some(steer),
        hook,
        pane_exited: exited,
    })
}

#[test]
fn a_turn_its_prompt_started_ends_it_however_soon() {
    for (at, turn) in [(1_001, 1_001), (1_500, 1_200), (60_000, 1_001)] {
        let h = rec(HookSignal::Idle, at, Some(turn));
        assert_eq!(
            decide(prompt(Some(1_000)), Some(h), false),
            Some((7, TurnOutcome::Idle))
        );
    }
}

#[test]
fn the_end_of_a_turn_running_before_the_prompt_does_not_end_it() {
    let earlier = rec(HookSignal::Idle, 2_000, Some(900));
    assert_eq!(decide(prompt(Some(1_000)), Some(earlier), false), None);
    let blocked = rec(HookSignal::Blocked, 2_000, Some(1_000));
    assert_eq!(decide(prompt(Some(1_000)), Some(blocked), false), None);
    let own = rec(HookSignal::Blocked, 3_000, Some(2_500));
    assert_eq!(
        decide(prompt(Some(1_000)), Some(own), false),
        Some((7, TurnOutcome::NeedsInput))
    );
}

#[test]
fn only_box_clock_times_are_compared() {
    // The Box clock may run seconds behind or ahead of the controller; nothing here carries a
    // controller time, so a report of the prompt's turn counts whatever the skew.
    let h = rec(HookSignal::Idle, 5, Some(3));
    assert_eq!(
        decide(prompt(Some(2)), Some(h), false),
        Some((7, TurnOutcome::Idle))
    );
}

#[test]
fn a_report_naming_no_turn_does_not_end_a_prompt() {
    let h = rec(HookSignal::Idle, 5_000, None);
    assert_eq!(decide(prompt(Some(1_000)), Some(h), false), None);
}

#[test]
fn a_prompt_with_no_report_before_it_ends_on_any_named_turn() {
    let h = rec(HookSignal::Idle, 5, Some(4));
    assert_eq!(
        decide(prompt(None), Some(h), false),
        Some((7, TurnOutcome::Idle))
    );
}

#[test]
fn a_menu_choice_ends_on_the_next_report_of_the_shown_turn() {
    let h = rec(HookSignal::Idle, 1_500, Some(900));
    assert_eq!(
        decide(choice(1_000), Some(h), false),
        Some((7, TurnOutcome::Idle))
    );
    for at in [1_000, 999] {
        let h = rec(HookSignal::Idle, at, Some(900));
        assert_eq!(decide(choice(1_000), Some(h), false), None);
    }
}

#[test]
fn no_unanswered_steer_is_none() {
    let f = TurnFacts {
        steer: None,
        hook: Some(rec(HookSignal::Idle, 500, Some(500))),
        pane_exited: true,
    };
    assert_eq!(turn_done(&f), None);
}

#[test]
fn exited_pane_wins_over_hook() {
    let h = rec(HookSignal::Idle, 500, Some(500));
    assert_eq!(
        decide(prompt(Some(1_000)), Some(h), true),
        Some((7, TurnOutcome::Exited))
    );
    assert_eq!(
        decide(prompt(Some(1_000)), None, true),
        Some((7, TurnOutcome::Exited))
    );
}

#[test]
fn busy_or_starting_is_none() {
    let h = rec(HookSignal::Other, 5_000, Some(4_000));
    assert_eq!(decide(prompt(Some(1_000)), Some(h), false), None);
}

#[test]
fn no_hook_is_none() {
    assert_eq!(decide(prompt(Some(1_000)), None, false), None);
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
