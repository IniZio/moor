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

/// Position of a state in `TaskState::ALL`; the exhaustive match fails to compile on a new variant.
const fn state_index(s: TaskState) -> usize {
    match s {
        S::Checking => 0,
        S::Rejected => 1,
        S::WaitingForBox => 2,
        S::SettingUp => 3,
        S::SetupFailed => 4,
        S::Working => 5,
        S::Idle => 6,
        S::Asleep => 7,
        S::Closed => 8,
    }
}

/// Position of an input in `all_inputs()`; the exhaustive match fails to compile on a new variant.
const fn input_index(i: &TaskInput) -> usize {
    match i {
        I::Insert => 0,
        I::Rejected { .. } => 1,
        I::AtCapacity { .. } => 2,
        I::Admitted => 3,
        I::Promoted => 4,
        I::BoxReady { .. } => 5,
        I::AcquireFailed { .. } => 6,
        I::CredFailed { .. } => 7,
        I::DotfilesFailed { .. } => 8,
        I::Retry => 9,
        I::AgentQuiet => 10,
        I::AgentBusy => 11,
        I::RunStarted => 12,
        I::SleepDue => 13,
        I::SleepCmd => 14,
        I::Wake => 15,
        I::Close => 16,
    }
}

/// `ALL` lists every state once, in match order.
#[test]
fn all_states_listed_once_in_order() {
    assert_eq!(TaskState::ALL.len(), 9);
    for (pos, s) in TaskState::ALL.into_iter().enumerate() {
        assert_eq!(state_index(s), pos, "{s:?}");
    }
}

/// The hand-written name equals the serde name and parses back.
#[test]
fn state_names_match_serde_and_parse() {
    for s in TaskState::ALL {
        let json = serde_json::to_value(s).unwrap();
        assert_eq!(json.as_str(), Some(s.as_str()), "{s:?}");
        assert_eq!(TaskState::parse(s.as_str()), Some(s), "{s:?}");
    }
}

/// The sample inputs cover every variant once, with unique names.
#[test]
fn input_samples_cover_every_variant_with_unique_names() {
    let all = all_inputs();
    assert_eq!(all.len(), 17);
    for (pos, i) in all.iter().enumerate() {
        assert_eq!(input_index(i), pos, "{i:?}");
    }
    let mut names: Vec<_> = all.iter().map(TaskInput::name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
}
