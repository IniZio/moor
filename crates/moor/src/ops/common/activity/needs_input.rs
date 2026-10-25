//! The `needs_input` event written once per waiting episode.

use moor_core::activity::{Activity, AgentState, Source};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::Event;

use super::Reading;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Append a `needs_input` event when the agent starts waiting for the user.
///
/// A hook-reported wait gets one event per hook record, so a `moor run` meanwhile does not end
/// it. A pane-read wait ends when the Box turns busy or working, so its event is skipped while
/// the latest such marker is a `needs_input` with the same question and options. It is written
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
    let hooked = matches!(source, Source::Hooks | Source::HooksCached);
    let cache = if hooked { store.hook(row.id)? } else { None };
    if cache.as_ref().is_some_and(|c| c.reported) {
        return Ok(());
    }
    let event = Event::NeedsInput {
        question: question.clone(),
        options: options.clone(),
    };
    if cache.is_none() {
        let last = store.last_waiting_marker(row.id)?;
        if last.is_some_and(|l| l.kind == event.kind() && l.payload == event.payload()) {
            return Ok(());
        }
    }
    store.apply(Decision::new(row.id).event(ExtraEvent::NeedsInput {
        question: question.clone(),
        options: options.clone(),
    }))?;
    if let Some(c) = cache {
        store.mark_hook_reported(row.id, c.record.at)?;
    }
    Ok(())
}
