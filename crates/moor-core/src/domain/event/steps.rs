//! Events derived from the separate Box lifecycle and Session inputs.

use super::Event;
use crate::domain::box_state::{BoxLifecycle, BoxLifecycleInput, SetupOutcome};
use crate::domain::session_state::SessionInput;

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
        BoxLifecycleInput::AtCapacity { max_boxes } => vec![Event::Pending {
            max_boxes: *max_boxes,
        }],
        BoxLifecycleInput::Admitted | BoxLifecycleInput::Promoted | BoxLifecycleInput::Retry => {
            vec![Event::SettingUp { from: origin }]
        }
        BoxLifecycleInput::Ready { setup } => match setup {
            SetupOutcome::Ok => vec![Event::Working],
            SetupOutcome::Failed { line } => {
                vec![Event::Working, setup_failed(line.clone())]
            }
        },
        BoxLifecycleInput::WorkingSetupFailed { cause }
        | BoxLifecycleInput::SetupFailed { cause, which: None } => {
            vec![setup_failed(cause.clone())]
        }
        BoxLifecycleInput::SetupFailed {
            cause,
            which: Some(which),
        } => vec![
            Event::CredentialFailed {
                which: which.clone(),
                error: cause.clone(),
            },
            setup_failed(cause.clone()),
        ],
        BoxLifecycleInput::Sleep { auto, reason } => vec![Event::Asleep {
            auto: *auto,
            reason: reason.clone(),
            cache_save: None,
        }],
        BoxLifecycleInput::Wake => vec![Event::Woke],
        BoxLifecycleInput::Suspend { reason } => vec![Event::Asleep {
            auto: true,
            reason: Some(reason.clone()),
            cache_save: None,
        }],
        BoxLifecycleInput::WaitForRoom => vec![],
        BoxLifecycleInput::Close { reason } => vec![Event::Closed {
            from: origin,
            reason: reason.clone(),
        }],
    }
}

/// A `setup_failed` event for `line` that says nothing more.
fn setup_failed(line: impl Into<String>) -> Event {
    Event::SetupFailed {
        line: line.into(),
        why: None,
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
