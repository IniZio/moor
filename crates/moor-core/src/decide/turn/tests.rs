//! Tests for the turn decider.

use super::{turn_done, SteerMark, TermTurn, TurnFacts, TurnSignal};
use crate::domain::event::TurnOutcome;

fn mark(turn_before: Option<u64>) -> SteerMark {
    SteerMark { id: 7, turn_before }
}

const fn term(signal: TurnSignal, turn: u64) -> TermTurn {
    TermTurn { signal, turn }
}

fn decide(steer: SteerMark, term: Option<TermTurn>, exited: bool) -> Option<(i64, TurnOutcome)> {
    turn_done(&TurnFacts {
        steer: Some(steer),
        term,
        exited,
    })
}

#[test]
fn a_turn_past_the_mark_ends_the_steer_as_its_signal_says() {
    assert_eq!(
        decide(mark(Some(2)), Some(term(TurnSignal::Idle, 3)), false),
        Some((7, TurnOutcome::Idle))
    );
    assert_eq!(
        decide(mark(Some(2)), Some(term(TurnSignal::Blocked, 3)), false),
        Some((7, TurnOutcome::NeedsInput))
    );
}

#[test]
fn a_quiet_terminal_still_at_the_mark_has_not_begun_the_turn() {
    assert_eq!(
        decide(mark(Some(3)), Some(term(TurnSignal::Idle, 3)), false),
        None
    );
}

#[test]
fn a_working_terminal_never_ends_a_turn() {
    assert_eq!(
        decide(mark(Some(1)), Some(term(TurnSignal::Other, 5)), false),
        None
    );
}

#[test]
fn an_exit_ends_the_turn_whatever_the_terminal_says() {
    assert_eq!(
        decide(mark(Some(9)), None, true),
        Some((7, TurnOutcome::Exited))
    );
}

#[test]
fn a_steer_with_no_mark_ends_at_the_first_quiet_terminal() {
    assert_eq!(
        decide(mark(None), Some(term(TurnSignal::Idle, 0)), false),
        Some((7, TurnOutcome::Idle))
    );
}

#[test]
fn no_steer_or_no_read_decides_nothing() {
    assert_eq!(decide(mark(Some(1)), None, false), None);
    assert_eq!(
        turn_done(&TurnFacts {
            steer: None,
            term: Some(term(TurnSignal::Idle, 5)),
            exited: false,
        }),
        None
    );
}
