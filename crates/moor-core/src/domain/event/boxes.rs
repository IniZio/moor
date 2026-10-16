//! Events a Box step produces.

use super::Event;
use crate::domain::boxes::{BoxInput, BoxState};
use crate::name::BoxHandle;

/// The events a box step produces; `handle` is the Box row's current handle.
#[must_use]
pub fn box_events(
    _from: Option<BoxState>,
    _to: BoxState,
    input: &BoxInput,
    handle: Option<&BoxHandle>,
) -> Vec<Event> {
    let text = |h: Option<&BoxHandle>| h.map(|h| h.as_str().to_owned());
    match input {
        BoxInput::Requested => vec![Event::BoxRequested],
        BoxInput::Acquired { .. } | BoxInput::Wake => vec![Event::BoxReady],
        BoxInput::AcquireFailed { cause, attempted } => vec![Event::BoxFailed {
            error: cause.clone(),
            handle: text(attempted.as_ref().or(handle)),
        }],
        BoxInput::Sleep => vec![Event::BoxAsleep],
        BoxInput::DriverMissing => vec![Event::BoxGone],
        BoxInput::Released => vec![Event::BoxDeleted],
        BoxInput::ReleaseErr { cause } => vec![Event::BoxReleaseFailed {
            error: cause.clone(),
            handle: text(handle),
        }],
    }
}
