//! Tests for the turn decider.

use super::{turn_done, SteerMark, TermTurn, TurnFacts, TurnSignal};
use crate::domain::event::TurnOutcome;

fn mark(turn_before: Option<u64>) -> SteerMark {
    SteerMark {
        id: 7,
        turn_before,
        status_before: None,
    }
}

const fn term(signal: TurnSignal, turn: u64) -> TermTurn {
    TermTurn {
        signal,
        turn,
        status_at: None,
    }
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

/// A steer sent to an agent not working whose status was set at 100, after 3 ended turns.
fn sent_to_a_quiet_agent() -> SteerMark {
    SteerMark {
        status_before: Some(100),
        ..mark(Some(3))
    }
}

const fn at(signal: TurnSignal, turn: u64, status_at: i64) -> TermTurn {
    TermTurn {
        signal,
        turn,
        status_at: Some(status_at),
    }
}

#[test]
fn a_later_idle_status_ends_the_turn_though_no_turn_ended() {
    assert_eq!(
        decide(
            sent_to_a_quiet_agent(),
            Some(at(TurnSignal::Idle, 3, 200)),
            false
        ),
        Some((7, TurnOutcome::Idle))
    );
}

#[test]
fn the_status_seen_at_the_send_does_not_end_the_turn() {
    for signal in [TurnSignal::Idle, TurnSignal::Blocked] {
        assert_eq!(
            decide(sent_to_a_quiet_agent(), Some(at(signal, 3, 100)), false),
            None
        );
    }
}

#[test]
fn a_later_working_or_blocked_status_does_not_end_the_turn_early() {
    for signal in [TurnSignal::Other, TurnSignal::Blocked] {
        assert_eq!(
            decide(sent_to_a_quiet_agent(), Some(at(signal, 3, 200)), false),
            None,
            "{signal:?}"
        );
    }
}

#[test]
fn a_steer_queued_behind_a_running_turn_waits_for_its_own_turn() {
    assert_eq!(
        decide(mark(Some(4)), Some(at(TurnSignal::Idle, 4, 200)), false),
        None
    );
}
