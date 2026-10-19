//! Tests for the Box state machine.

use super::{box_may_sleep, BoxLifecycle, BoxLifecycleInput, SetupOutcome, StepError};
use crate::domain::session_state::SessionState;
use crate::domain::Illegal;

use BoxLifecycle as S;
use BoxLifecycleInput as I;
use SessionState as SS;

/// One of every input variant; `Sleep` is by command.
fn all_inputs() -> Vec<BoxLifecycleInput> {
    vec![
        I::Insert,
        I::Rejected {
            reason: "x".to_owned(),
        },
        I::AtCapacity { max_boxes: 2 },
        I::Admitted,
        I::Promoted,
        I::Ready {
            setup: SetupOutcome::Ok,
        },
        I::SetupFailed {
            cause: "x".to_owned(),
            which: None,
        },
        I::Retry,
        I::Sleep { auto: false },
        I::Wake,
        I::Close,
    ]
}

/// Every legal (from, input, to) triple.
fn legal() -> Vec<(BoxLifecycle, BoxLifecycleInput, BoxLifecycle)> {
    let x = || "x".to_owned();
    let mut table = vec![
        (S::Checking, I::Rejected { reason: x() }, S::Rejected),
        (
            S::Checking,
            I::AtCapacity { max_boxes: 3 },
            S::WaitingForBox,
        ),
        (S::Checking, I::Admitted, S::SettingUp),
        (S::WaitingForBox, I::Promoted, S::SettingUp),
        (
            S::SettingUp,
            I::Ready {
                setup: SetupOutcome::Ok,
            },
            S::Ready,
        ),
        (
            S::SettingUp,
            I::Ready {
                setup: SetupOutcome::Failed { line: x() },
            },
            S::Ready,
        ),
        (
            S::SettingUp,
            I::SetupFailed {
                cause: x(),
                which: None,
            },
            S::SetupFailed,
        ),
        (
            S::SettingUp,
            I::SetupFailed {
                cause: x(),
                which: Some(x()),
            },
            S::SetupFailed,
        ),
        (S::SetupFailed, I::Retry, S::SettingUp),
        (S::Ready, I::Sleep { auto: false }, S::Asleep),
        (S::Ready, I::Sleep { auto: true }, S::Asleep),
        (S::Asleep, I::Wake, S::Ready),
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
            from.step(&[SS::Idle, SS::Idle], &input),
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
                from.step(&[SS::Idle], &input),
                Err(StepError::Illegal(Illegal {
                    from: from.as_str().to_owned(),
                    input: input.name().to_owned(),
                })),
                "{} + {}",
                from.as_str(),
                input.name()
            );
        }
    }
}

#[test]
fn auto_does_not_change_the_sleep_result() {
    for sessions in [vec![SS::Idle], vec![SS::Working]] {
        assert_eq!(
            S::Ready.step(&sessions, &I::Sleep { auto: true }),
            S::Ready.step(&sessions, &I::Sleep { auto: false })
        );
    }
}

#[test]
fn legal_table_has_expected_size() {
    assert_eq!(legal().len(), 12 + 6);
}

#[test]
fn sleep_is_refused_while_any_session_is_not_idle() {
    for busy in [SS::Starting, SS::Working] {
        let input = I::Sleep { auto: false };
        assert_eq!(
            S::Ready.step(&[SS::Idle, busy], &input),
            Err(StepError::SleepBlocked { busy: 1 }),
            "{busy:?}"
        );
    }
}

#[test]
fn blocked_sleep_differs_from_sleep_in_the_wrong_state() {
    let input = I::Sleep { auto: false };
    let sessions = [SS::Working, SS::Starting, SS::Idle, SS::Ended];
    assert_eq!(
        S::Ready.step(&sessions, &input),
        Err(StepError::SleepBlocked { busy: 2 })
    );
    assert!(matches!(
        S::Asleep.step(&sessions, &input),
        Err(StepError::Illegal(_))
    ));
    assert!(matches!(
        S::SettingUp.step(&sessions, &input),
        Err(StepError::Illegal(_))
    ));
}

#[test]
fn setup_outcome_survives_the_step_and_feeds_events() {
    let failed = I::Ready {
        setup: SetupOutcome::Failed {
            line: "npm ci failed".to_owned(),
        },
    };
    assert_eq!(S::SettingUp.step(&[], &failed), Ok(S::Ready));
    assert!(matches!(
        &failed,
        I::Ready { setup: SetupOutcome::Failed { line } } if line == "npm ci failed"
    ));

    let cred = I::SetupFailed {
        cause: "denied".to_owned(),
        which: Some("api_key".to_owned()),
    };
    assert_eq!(S::SettingUp.step(&[], &cred), Ok(S::SetupFailed));
    assert!(matches!(
        &cred,
        I::SetupFailed { cause, which: Some(w) } if cause == "denied" && w == "api_key"
    ));
}

#[test]
fn closed_and_rejected_are_terminal() {
    for from in [S::Closed, S::Rejected] {
        assert!(!from.is_live());
        for input in all_inputs() {
            assert!(
                from.step(&[], &input).is_err(),
                "{} + {}",
                from.as_str(),
                input.name()
            );
        }
    }
}

#[test]
fn initial_accepts_only_insert() {
    for input in all_inputs() {
        let got = BoxLifecycle::initial(&input);
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
fn names_parse_and_serde_agree() {
    assert_eq!(S::ALL.len(), 8);
    for state in S::ALL {
        assert_eq!(BoxLifecycle::parse(state.as_str()), Some(state));
        let json = serde_json::to_value(state).unwrap();
        assert_eq!(json.as_str(), Some(state.as_str()));
    }
    assert_eq!(BoxLifecycle::parse("nope"), None);
}

#[test]
fn input_names_are_unique_snake_case() {
    let mut names: Vec<_> = all_inputs().iter().map(BoxLifecycleInput::name).collect();
    for n in &names {
        assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{n}");
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all_inputs().len());
}

#[test]
fn box_may_sleep_needs_every_session_idle() {
    assert!(box_may_sleep(&[]));
    assert!(box_may_sleep(&[SS::Idle]));
    assert!(box_may_sleep(&[SS::Idle, SS::Idle, SS::Idle]));
    assert!(!box_may_sleep(&[SS::Working]));
    assert!(!box_may_sleep(&[SS::Starting]));
    assert!(!box_may_sleep(&[SS::Idle, SS::Working]));
    assert!(!box_may_sleep(&[SS::Starting, SS::Idle]));
}

#[test]
fn ended_sessions_do_not_block_sleep() {
    assert!(box_may_sleep(&[SS::Ended]));
    assert!(box_may_sleep(&[SS::Ended, SS::Idle]));
    assert!(!box_may_sleep(&[SS::Ended, SS::Working]));
}
