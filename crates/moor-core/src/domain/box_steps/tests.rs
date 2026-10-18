use super::*;
use crate::domain::task::SetupOutcome;

fn inputs() -> Vec<TaskInput> {
    vec![
        TaskInput::Insert,
        TaskInput::Rejected { reason: "r".into() },
        TaskInput::AtCapacity { max_boxes: 1 },
        TaskInput::Admitted,
        TaskInput::Promoted,
        TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        },
        TaskInput::BoxReady {
            setup: SetupOutcome::Failed { line: "l".into() },
        },
        TaskInput::AcquireFailed { cause: "c".into() },
        TaskInput::CredFailed {
            which: "w".into(),
            error: "e".into(),
        },
        TaskInput::DotfilesFailed { error: "e".into() },
        TaskInput::SyncFailed { error: "e".into() },
        TaskInput::Retry,
        TaskInput::AgentQuiet,
        TaskInput::AgentBusy,
        TaskInput::RunStarted,
        TaskInput::SleepDue,
        TaskInput::SleepCmd,
        TaskInput::Wake,
        TaskInput::Close,
    ]
}

fn parts(state: TaskState) -> (BoxLifecycle, SessionState) {
    match state {
        TaskState::Checking => (BoxLifecycle::Checking, SessionState::Starting),
        TaskState::Rejected => (BoxLifecycle::Rejected, SessionState::Starting),
        TaskState::WaitingForBox => (BoxLifecycle::WaitingForBox, SessionState::Starting),
        TaskState::SettingUp => (BoxLifecycle::SettingUp, SessionState::Starting),
        TaskState::SetupFailed => (BoxLifecycle::SetupFailed, SessionState::Starting),
        TaskState::Working => (BoxLifecycle::Ready, SessionState::Working),
        TaskState::Idle => (BoxLifecycle::Ready, SessionState::Idle),
        TaskState::Asleep => (BoxLifecycle::Asleep, SessionState::Idle),
        TaskState::Closed => (BoxLifecycle::Closed, SessionState::Ended),
    }
}

#[test]
fn matches_task_state_step_for_every_pair() {
    for state in TaskState::ALL {
        let (lifecycle, primary) = parts(state);
        assert_eq!(summary(lifecycle, primary), state);
        for input in inputs() {
            let want = state.step(&input).map_err(Some);
            let got = step(lifecycle, primary, &[], &input)
                .map(|(lc, s)| summary(lc, s))
                .map_err(|e| match e {
                    StepError::Illegal(i) => Some(i),
                    StepError::SleepBlocked { .. } => None,
                });
            assert_eq!(got, want, "{state:?} + {input:?}");
        }
    }
}

#[test]
fn sleep_due_blocked_by_working_other() {
    let got = step(
        BoxLifecycle::Ready,
        SessionState::Idle,
        &[SessionState::Working],
        &TaskInput::SleepDue,
    );
    assert_eq!(got, Err(StepError::SleepBlocked { busy: 1 }));
}

#[test]
fn sleep_due_allowed_with_idle_or_ended_others() {
    let got = step(
        BoxLifecycle::Ready,
        SessionState::Idle,
        &[SessionState::Idle, SessionState::Ended],
        &TaskInput::SleepDue,
    );
    assert_eq!(got, Ok((BoxLifecycle::Asleep, SessionState::Idle)));
}

#[test]
fn initial_insert_checks_and_starts() {
    assert_eq!(
        initial(&TaskInput::Insert),
        Ok((BoxLifecycle::Checking, SessionState::Starting))
    );
}

#[test]
fn initial_rejects_other_inputs() {
    let err = initial(&TaskInput::Admitted).unwrap_err();
    assert_eq!(err, TaskState::initial(&TaskInput::Admitted).unwrap_err());
}
