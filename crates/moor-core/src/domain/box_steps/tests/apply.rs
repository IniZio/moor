//! `apply` over separate Box and Session inputs gives the same states and events as `step`.
#![allow(clippy::panic, reason = "test assertions")]

use super::*;
use crate::domain::box_state::BoxLifecycleInput as L;
use crate::domain::event::{lifecycle_events, session_events, task_events, Event};
use crate::domain::session_state::SessionInput as S;
use crate::domain::task::SetupOutcome;

/// The Box lifecycle and Session inputs an old input stands for.
fn pair(input: &TaskInput) -> (Option<L>, Option<S>) {
    let setup_failed = |cause: &String, which: Option<&String>| {
        Some(L::SetupFailed {
            cause: cause.clone(),
            which: which.cloned(),
        })
    };
    match input {
        TaskInput::Insert => (Some(L::Insert), Some(S::Open)),
        TaskInput::Rejected { reason } => (
            Some(L::Rejected {
                reason: reason.clone(),
            }),
            None,
        ),
        TaskInput::AtCapacity { max_boxes } => (
            Some(L::AtCapacity {
                max_boxes: *max_boxes,
            }),
            None,
        ),
        TaskInput::Admitted => (Some(L::Admitted), None),
        TaskInput::Promoted => (Some(L::Promoted), None),
        TaskInput::Retry => (Some(L::Retry), None),
        TaskInput::BoxReady { setup } => (
            Some(L::Ready {
                setup: setup.clone(),
            }),
            Some(S::Started),
        ),
        TaskInput::AcquireFailed { cause } => (setup_failed(cause, None), None),
        TaskInput::DotfilesFailed { error } | TaskInput::SyncFailed { error } => {
            (setup_failed(error, None), None)
        }
        TaskInput::CredFailed { which, error } => (setup_failed(error, Some(which)), None),
        TaskInput::AgentQuiet => (None, Some(S::AgentQuiet)),
        TaskInput::AgentBusy => (None, Some(S::AgentBusy)),
        TaskInput::RunStarted => (None, Some(S::RunStarted)),
        TaskInput::SleepDue => (Some(L::Sleep { auto: true }), None),
        TaskInput::SleepCmd => (Some(L::Sleep { auto: false }), None),
        TaskInput::Wake => (Some(L::Wake), Some(S::AgentBusy)),
        TaskInput::Close => (Some(L::Close), Some(S::End)),
    }
}

/// An old event with its origin state named by the Box lifecycle.
fn convert(event: Event) -> Event<BoxLifecycle> {
    match event {
        Event::Started => Event::Started,
        Event::Rejected { reason } => Event::Rejected { reason },
        Event::WaitingForBox { max_boxes } => Event::WaitingForBox { max_boxes },
        Event::SettingUp { from } => Event::SettingUp {
            from: parts(from).0,
        },
        Event::Working => Event::Working,
        Event::SetupFailed { line } => Event::SetupFailed { line },
        Event::CredentialFailed { which, error } => Event::CredentialFailed { which, error },
        Event::Idled => Event::Idled,
        Event::Busy => Event::Busy,
        Event::Asleep { auto } => Event::Asleep { auto },
        Event::Woke => Event::Woke,
        Event::Closed { from } => Event::Closed {
            from: parts(from).0,
        },
        other => panic!("step never produces {other:?}"),
    }
}

fn convert_all(events: Vec<Event>) -> Vec<Event<BoxLifecycle>> {
    events.into_iter().map(convert).collect()
}

fn other_sessions() -> [Vec<SessionState>; 4] {
    [
        vec![],
        vec![SessionState::Ended],
        vec![SessionState::Working],
        vec![SessionState::Idle, SessionState::Starting],
    ]
}

#[test]
fn apply_matches_old_step_for_every_pair() {
    let mut cases = 0;
    for state in TaskState::ALL {
        let (lifecycle, primary) = parts(state);
        for others in other_sessions() {
            for input in inputs() {
                if input == TaskInput::Insert {
                    continue;
                }
                cases += 1;
                let (l, s) = pair(&input);
                let want = step(lifecycle, primary, &others, &input);
                let got = apply(lifecycle, primary, &others, l.as_ref(), s.as_ref());
                let ctx = format!("{state:?} + {others:?} + {input:?}");
                match (&want, &got) {
                    (Ok(a), Ok(b)) => assert_eq!(a, b, "{ctx}"),
                    (Err(StepError::Illegal(_)), Err(StepError::Illegal(_))) => {}
                    (Err(a @ StepError::SleepBlocked { .. }), Err(b)) => {
                        assert_eq!(a, b, "{ctx}");
                    }
                    _ => panic!("{ctx}: old {want:?}, new {got:?}"),
                }
            }
        }
    }
    assert_eq!(cases, 9 * 4 * 18);
}

#[test]
fn apply_events_match_old_events_for_every_ok_pair() {
    let mut compared = 0;
    for state in TaskState::ALL {
        let (lifecycle, primary) = parts(state);
        for others in other_sessions() {
            for input in inputs() {
                if input == TaskInput::Insert {
                    continue;
                }
                let (l, s) = pair(&input);
                let Ok((new_lc, new_primary)) =
                    apply(lifecycle, primary, &others, l.as_ref(), s.as_ref())
                else {
                    continue;
                };
                let got = match (&l, &s) {
                    (Some(l), _) => lifecycle_events(Some(lifecycle), new_lc, l),
                    (None, Some(s)) => session_events(s),
                    (None, None) => unreachable!("every input maps to one side"),
                };
                let want = convert_all(task_events(
                    Some(state),
                    summary(new_lc, new_primary),
                    &input,
                ));
                assert_eq!(got, want, "{state:?} + {others:?} + {input:?}");
                compared += 1;
            }
        }
    }
    assert!(compared > 0);
}

#[test]
fn apply_insert_matches_old_initial() {
    let (lc, session) = initial(&TaskInput::Insert).unwrap();
    assert_eq!(lc, BoxLifecycle::initial(&L::Insert).unwrap());
    assert_eq!(session, SessionState::initial(&S::Open).unwrap());

    let got = lifecycle_events(None, lc, &L::Insert);
    let want = convert_all(task_events(None, summary(lc, session), &TaskInput::Insert));
    assert_eq!(got, want);
}

#[test]
fn apply_without_inputs_changes_nothing() {
    for state in TaskState::ALL {
        let (lifecycle, primary) = parts(state);
        let got = apply(lifecycle, primary, &[], None, None);
        assert_eq!(got, Ok((lifecycle, primary)), "{state:?}");
    }
}

#[test]
fn apply_setup_outcome_failed_matches_old() {
    let input = TaskInput::BoxReady {
        setup: SetupOutcome::Failed { line: "l".into() },
    };
    let (l, s) = pair(&input);
    let got = apply(
        BoxLifecycle::SettingUp,
        SessionState::Starting,
        &[],
        l.as_ref(),
        s.as_ref(),
    );
    assert_eq!(got, Ok((BoxLifecycle::Ready, SessionState::Working)));
}
