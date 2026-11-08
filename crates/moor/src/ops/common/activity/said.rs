//! The agent's last message, read from the note its hooks left when it went idle.

use moor_core::activity::{Activity, AgentState, Probe, Source};
use moor_core::domain::decision::{Decision, ExtraEvent};

use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Most characters of the agent's last message kept. Both ends are kept: a reply opens with its
/// summary and closes with its result, and a long middle is usually pasted command output.
const SAID_MAX: usize = 4000;
/// Stands for the characters cut from the middle.
const ELIDED: &str = "\n…\n";

/// The agent's last message, cut in the middle, when it reports idle and its hooks left one.
pub(super) fn said(activity: &Activity, probe: &Probe) -> Option<String> {
    if activity.source != Source::Status
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

/// Records `said` ahead of a step that quiets the Box. False, with nothing written, when another
/// settle stepped the Box after `row` was read: that settle recorded the message already.
pub(super) fn record(
    store: &Store,
    row: &BoxRow,
    quiet: bool,
    said: Option<&str>,
) -> Result<bool, CommandError> {
    if store
        .box_row(row.id)?
        .is_none_or(|now| now.session != row.session)
    {
        return Ok(false);
    }
    if let (true, Some(text)) = (quiet, said) {
        let text = text.to_owned();
        store.apply(Decision::new(row.id).event(ExtraEvent::AgentSaid { text }))?;
    }
    Ok(true)
}
