//! The agent's last message, read from the note its hooks left when it went idle.

use moor_core::activity::{Activity, AgentState, Probe, Source};

use super::Reading;
use crate::error::CommandError;
use crate::store::Store;

/// Most characters of the agent's last message kept. Both ends are kept: a reply opens with its
/// summary and closes with its result, and a long middle is usually pasted command output.
const SAID_MAX: usize = 4000;
/// Stands for the characters cut from the middle.
const ELIDED: &str = "\n…\n";

/// The agent's last message, cut in the middle, when it reports idle after a turn of its own and
/// its hooks left one. Before its terminal has ended a turn, a message in the note was left by an
/// agent before it.
pub(super) fn said(activity: &Activity, probe: &Probe) -> Option<String> {
    let ended_a_turn = probe.term().is_some_and(|t| t.turn > 0);
    if activity.source != Source::Status
        || !ended_a_turn
        || !matches!(
            activity.state,
            AgentState::Idle | AgentState::Waiting { .. }
        )
    {
        return None;
    }
    probe
        .note()
        .and_then(|n| n.said.as_deref())
        .map(|text| ends(text, SAID_MAX))
}

/// `text` when it has at most `max` characters; otherwise its start and end around [`ELIDED`],
/// `max` characters in all.
fn ends(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let keep = max.saturating_sub(ELIDED.chars().count());
    let head: String = text.chars().take(keep / 2).collect();
    let tail: String = text.chars().skip(count - (keep - keep / 2)).collect();
    format!("{head}{ELIDED}{tail}")
}

/// The reply `reading` saw, once per idle status the agent reported, so neither a run meanwhile
/// nor a second settle of the same status says it again.
pub(super) fn reply(store: &Store, reading: &Reading) -> Result<Option<String>, CommandError> {
    let Some(text) = reading.said.as_ref() else {
        return Ok(None);
    };
    let since = reading.term.as_ref().and_then(|t| t.status_at);
    if let (Some(launch), Some(at)) = (reading.launch, since) {
        if store.note_said(launch, at)? {
            return Ok(None);
        }
    }
    Ok(Some(text.clone()))
}
