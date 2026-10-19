//! Events derived from the separate Box lifecycle and Session inputs.

use super::Event;
use crate::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use crate::domain::session_state::SessionInput;
use crate::domain::task::{SetupOutcome, TaskState};

/// The events a Box lifecycle step produces. A decision with a lifecycle input takes its
/// events from here only; [`session_events`] applies only when there is no lifecycle input.
#[must_use]
pub fn lifecycle_events(
    from: Option<BoxLifecycle>,
    to: BoxLifecycle,
    input: &BoxLifecycleInput,
) -> Vec<Event> {
    let origin = from.unwrap_or(to);
    match input {
        BoxLifecycleInput::Insert => vec![Event::Started],
        BoxLifecycleInput::Rejected { reason } => vec![Event::Rejected {
            reason: reason.clone(),
        }],
        BoxLifecycleInput::AtCapacity { max_boxes } => vec![Event::WaitingForBox {
            max_boxes: *max_boxes,
        }],
        BoxLifecycleInput::Admitted | BoxLifecycleInput::Promoted | BoxLifecycleInput::Retry => {
            vec![Event::SettingUp { from: origin }]
        }
        BoxLifecycleInput::Ready { setup } => match setup {
            SetupOutcome::Ok => vec![Event::Working],
            SetupOutcome::Failed { line } => {
                vec![Event::Working, Event::SetupFailed { line: line.clone() }]
            }
        },
        BoxLifecycleInput::SetupFailed { cause, which: None } => {
            vec![Event::SetupFailed {
                line: cause.clone(),
            }]
        }
        BoxLifecycleInput::SetupFailed {
            cause,
            which: Some(which),
        } => vec![
            Event::CredentialFailed {
                which: which.clone(),
                error: cause.clone(),
            },
            Event::SetupFailed {
                line: cause.clone(),
            },
        ],
        BoxLifecycleInput::Sleep { auto } => vec![Event::Asleep { auto: *auto }],
        BoxLifecycleInput::Wake => vec![Event::Woke],
        BoxLifecycleInput::Close => vec![Event::Closed { from: origin }],
    }
}

/// The events a Session step produces, when the decision has no lifecycle input.
#[must_use]
pub fn session_events(input: &SessionInput) -> Vec<Event> {
    match input {
        SessionInput::Open | SessionInput::End => vec![],
        SessionInput::Started => vec![Event::Working],
        SessionInput::AgentQuiet => vec![Event::Idled],
        SessionInput::AgentBusy | SessionInput::RunStarted => vec![Event::Busy],
    }
}

#[cfg(test)]
mod tests;

/// The Box lifecycle a combined state stands for.
pub(super) const fn lifecycle_of(state: TaskState) -> BoxLifecycle {
    match state {
        TaskState::Checking => BoxLifecycle::Checking,
        TaskState::Rejected => BoxLifecycle::Rejected,
        TaskState::WaitingForBox => BoxLifecycle::WaitingForBox,
        TaskState::SettingUp => BoxLifecycle::SettingUp,
        TaskState::SetupFailed => BoxLifecycle::SetupFailed,
        TaskState::Working | TaskState::Idle => BoxLifecycle::Ready,
        TaskState::Asleep => BoxLifecycle::Asleep,
        TaskState::Closed => BoxLifecycle::Closed,
    }
}
