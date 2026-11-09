//! Events a Box step produces.

use super::Event;
use crate::domain::hands::{HandsInput, HandsState};
use crate::name::BoxHandle;

/// The events a box step produces; `handle` is the Box row's current handle.
#[must_use]
pub fn box_events(
    _from: Option<HandsState>,
    _to: HandsState,
    input: &HandsInput,
    handle: Option<&BoxHandle>,
) -> Vec<Event> {
    let text = |h: Option<&BoxHandle>| h.map(|h| h.as_str().to_owned());
    match input {
        HandsInput::Requested => vec![Event::BoxRequested],
        HandsInput::Acquired { .. } | HandsInput::Wake => vec![Event::BoxReady],
        HandsInput::AcquireFailed { cause, attempted } => vec![Event::BoxFailed {
            error: cause.clone(),
            handle: text(attempted.as_ref().or(handle)),
        }],
        HandsInput::Sleep => vec![Event::BoxAsleep],
        HandsInput::DriverMissing => vec![Event::BoxGone],
        HandsInput::Released => vec![Event::BoxDeleted],
        HandsInput::ReleaseErr { cause } => vec![Event::BoxReleaseFailed {
            error: cause.clone(),
            handle: text(handle),
        }],
    }
}
