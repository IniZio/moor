//! The `needs_input` event written once per waiting episode.

use moor_core::activity::{Activity, AgentState, Source};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::Event;

use super::Reading;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Append a `needs_input` event when the agent starts waiting for the user.
///
/// One event per status the agent reported: a `moor run` meanwhile does not end the episode,
/// and the agent asking again sets a new status. With no status time to go by, the event is
/// skipped while the Box's latest marker is a `needs_input` with the same question. It is written
/// before any state step so a refused step cannot lose it. A first-run stop gets none: the start
/// reports it, and typing into such a screen is never how moor gets past it.
pub(super) fn note_needs_input(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    let Some(Activity {
        state: AgentState::NeedsInput { question, options },
        source,
    }) = reading.activity.as_ref()
    else {
        return Ok(());
    };
    if *source == Source::FirstRun {
        return Ok(());
    }
    let event = Event::NeedsInput {
        question: question.clone(),
        options: options.clone(),
    };
    let since = reading.term.as_ref().and_then(|t| t.status_at);
    if let (Some(launch), Some(at)) = (reading.session, since) {
        if store.note_asked(launch, at)? {
            return Ok(());
        }
    } else {
        let last = store.last_waiting_marker(row.id)?;
        if last.is_some_and(|l| l.kind == event.kind() && l.payload == event.payload()) {
            return Ok(());
        }
    }
    store.apply(Decision::new(row.id).event(ExtraEvent::NeedsInput {
        question: question.clone(),
        options: options.clone(),
    }))?;
    Ok(())
}
