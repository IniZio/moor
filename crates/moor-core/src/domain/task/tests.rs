//! Tests for the Task state machine.

use super::{SetupOutcome, TaskInput, TaskState};
use crate::domain::Illegal;

use TaskInput as I;
use TaskState as S;

/// One of every input variant.
fn all_inputs() -> Vec<TaskInput> {
    let s = || "x".to_owned();
    vec![
        I::Insert,
        I::Rejected { reason: s() },
        I::AtCapacity { max_boxes: 2 },
        I::Admitted,
        I::Promoted,
        I::BoxReady {
            setup: SetupOutcome::Ok,
        },
        I::AcquireFailed { cause: s() },
        I::CredFailed {
            which: s(),
            error: s(),
        },
        I::DotfilesFailed { error: s() },
        I::Retry,
        I::AgentQuiet,
        I::AgentBusy,
        I::RunStarted,
        I::SleepDue,
        I::SleepCmd,
        I::Wake,
        I::Close,
    ]
}

/// Every legal (from, input, to) triple.
fn legal() -> Vec<(TaskState, TaskInput, TaskState)> {
    let s = || "x".to_owned();
    let failed = SetupOutcome::Failed { line: s() };
    let mut table = vec![
        (S::Checking, I::Rejected { reason: s() }, S::Rejected),
        (
            S::Checking,
            I::AtCapacity { max_boxes: 3 },
            S::WaitingForBox,
        ),
        (S::Checking, I::Admitted, S::SettingUp),
        (S::WaitingForBox, I::Promoted, S::SettingUp),
        (
            S::SettingUp,
            I::BoxReady {
                setup: SetupOutcome::Ok,
            },
            S::Working,
        ),
        (S::SettingUp, I::BoxReady { setup: failed }, S::Working),
        (
            S::SettingUp,
            I::AcquireFailed { cause: s() },
            S::SetupFailed,
        ),
        (
            S::SettingUp,
            I::CredFailed {
                which: s(),
                error: s(),
            },
            S::SetupFailed,
        ),
        (
            S::SettingUp,
            I::DotfilesFailed { error: s() },
            S::SetupFailed,
        ),
        (S::SetupFailed, I::Retry, S::SettingUp),
        (S::Working, I::AgentQuiet, S::Idle),
        (S::Idle, I::AgentBusy, S::Working),
        (S::Idle, I::RunStarted, S::Working),
        (S::Idle, I::SleepDue, S::Asleep),
        (S::Idle, I::SleepCmd, S::Asleep),
        (S::Asleep, I::Wake, S::Working),
    ];
    for state in S::ALL.into_iter().filter(|st| st.is_live()) {
        table.push((state, I::Close, S::Closed));
    }
    table
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
            if table
                .iter()
                .any(|(f, i, _)| *f == from && i.name() == input.name())
            {
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
fn legal_table_has_expected_size() {
    assert_eq!(legal().len(), 16 + 7);
}

#[test]
fn initial_accepts_only_insert() {
    for input in all_inputs() {
        let got = TaskState::initial(&input);
        if input == I::Insert {
            assert_eq!(got, Ok(S::Checking));
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
fn input_names_are_unique_snake_case() {
    let names: Vec<_> = all_inputs().iter().map(TaskInput::name).collect();
    for (i, n) in names.iter().enumerate() {
        assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{n}");
        assert!(!names.iter().skip(i + 1).any(|m| m == n), "dup {n}");
    }
    assert_eq!(I::AtCapacity { max_boxes: 1 }.name(), "at_capacity");
}

#[test]
fn as_str_parse_and_serde_agree() {
    for state in S::ALL {
        assert_eq!(TaskState::parse(state.as_str()), Some(state));
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, format!("\"{}\"", state.as_str()));
        assert_eq!(serde_json::from_str::<TaskState>(&json).unwrap(), state);
    }
    assert_eq!(TaskState::parse("nope"), None);
    assert_eq!(TaskState::parse(""), None);
}

#[test]
fn is_live_excludes_closed_and_rejected() {
    for state in S::ALL {
        assert_eq!(state.is_live(), !matches!(state, S::Closed | S::Rejected));
    }
}
