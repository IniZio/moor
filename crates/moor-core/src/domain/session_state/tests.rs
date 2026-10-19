//! Tests for the Session state machine.

use super::{SessionInput, SessionState};
use crate::domain::Illegal;

use SessionInput as I;
use SessionState as S;

/// One of every input variant.
fn all_inputs() -> Vec<SessionInput> {
    vec![
        I::Open,
        I::Started,
        I::AgentQuiet,
        I::AgentBusy,
        I::RunStarted,
        I::End,
    ]
}

/// Every legal (from, input, to) triple.
fn legal() -> Vec<(SessionState, SessionInput, SessionState)> {
    vec![
        (S::Starting, I::Started, S::Working),
        (S::Working, I::AgentQuiet, S::Idle),
        (S::Idle, I::AgentBusy, S::Working),
        (S::Idle, I::RunStarted, S::Working),
        (S::Starting, I::End, S::Ended),
        (S::Working, I::End, S::Ended),
        (S::Idle, I::End, S::Ended),
    ]
}

#[test]
fn every_legal_transition_steps() {
    for (from, input, to) in legal() {
        assert_eq!(
            from.step(&input),
            Ok(to),
            "{} + {}",
            from.as_str(),
            input.name()
        );
    }
}

#[test]
fn every_other_pair_is_illegal() {
    let table = legal();
    for from in S::ALL {
        for input in all_inputs() {
            if table.iter().any(|(f, i, _)| *f == from && *i == input) {
                continue;
            }
            assert_eq!(
                from.step(&input),
                Err(Illegal {
                    from: from.as_str().to_owned(),
                    input: input.name().to_owned(),
                }),
                "{} + {}",
                from.as_str(),
                input.name()
            );
        }
    }
}

#[test]
fn ended_is_terminal() {
    for input in all_inputs() {
        assert!(S::Ended.step(&input).is_err(), "{}", input.name());
    }
}

#[test]
fn initial_accepts_only_open() {
    for input in all_inputs() {
        let got = SessionState::initial(&input);
        if input == I::Open {
            assert_eq!(got, Ok(S::Starting));
        } else {
            assert_eq!(
                got,
                Err(Illegal {
                    from: "none".to_owned(),
                    input: input.name().to_owned()
                })
            );
        }
    }
}

#[test]
fn names_parse_and_serde_agree() {
    assert_eq!(S::ALL.len(), 4);
    for state in S::ALL {
        assert_eq!(SessionState::parse(state.as_str()), Some(state));
        let json = serde_json::to_value(state).unwrap();
        assert_eq!(json.as_str(), Some(state.as_str()));
    }
    assert_eq!(SessionState::parse("nope"), None);
}

#[test]
fn only_idle_and_ended_are_quiet() {
    for state in S::ALL {
        let want = matches!(state, S::Idle | S::Ended);
        assert_eq!(state.is_quiet(), want, "{}", state.as_str());
    }
}

#[test]
fn input_names_are_unique_snake_case() {
    let mut names: Vec<_> = all_inputs().iter().map(SessionInput::name).collect();
    for n in &names {
        assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{n}");
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all_inputs().len());
}
