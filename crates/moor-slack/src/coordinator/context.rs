//! The input of one turn, rebuilt from what the front keeps: the first request, the actions
//! the thread's turns took, the thread's Boxes, as much of the earlier exchange as fits the
//! budget, and the new items. The parts that change least come first.

use std::fmt::Write as _;

/// Longest first request shown.
const FIRST_MAX: usize = 8 * 1024;

/// Longest single message shown.
const ITEM_MAX: usize = 16 * 1024;

/// Longest action arguments shown.
const ARGS_MAX: usize = 200;

/// Bytes kept free for the line that counts the messages left out.
const OMITTED_LINE: usize = 64;

/// Who said something in the thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Speaker {
    /// A person, by Slack user id.
    Person(String),
    /// The front, in a fixed `[moor]` line.
    Front,
    /// The coordinator, in an earlier reply.
    Coordinator,
}

/// One message, notice or reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Said {
    /// Who said it.
    pub(crate) speaker: Speaker,
    /// What they said.
    pub(crate) text: String,
}

/// One action an earlier turn took, as the journal records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Action {
    /// The turn that took it.
    pub(crate) turn: i64,
    /// Slack user id of the person whose message the turn held.
    pub(crate) requested_by: String,
    /// The tool, such as `start_box`.
    pub(crate) tool: String,
    /// The arguments as text.
    pub(crate) args: String,
    /// What came of it; `None` when the call may or may not have run.
    pub(crate) outcome: Option<String>,
}

/// What one turn's context is built from.
pub(crate) struct Parts<'a> {
    /// The message that opened the thread, when it was recorded.
    pub(crate) first_request: Option<&'a str>,
    /// Every action the thread's turns took, oldest first.
    pub(crate) actions: &'a [Action],
    /// One line per Box of the thread.
    pub(crate) boxes: &'a [String],
    /// Earlier messages, notices and replies in thread order, without the first request.
    pub(crate) earlier: &'a [Said],
    /// What this turn takes, in Slack order.
    pub(crate) new: &'a [Said],
    /// For a turn that runs again one a restart cut off, the actions the cut turn took.
    pub(crate) rerun: Option<&'a [Action]>,
}

/// The context text, at most `budget` bytes unless the parts that always go in pass it on
/// their own. Earlier messages fill what is left, newest first, shown in thread order.
pub(crate) fn render(parts: &Parts<'_>, budget: usize) -> String {
    let mut head = String::from("## First request\n");
    head.push_str(&parts.first_request.map_or_else(
        || "(not recorded)".to_owned(),
        |text| cut(text, FIRST_MAX, "[the rest is not shown]"),
    ));
    head.push_str("\n\n## Actions this thread took\n");
    if parts.actions.is_empty() {
        head.push_str("None.\n");
    }
    for action in parts.actions {
        head.push_str(&action_line(action));
        head.push('\n');
    }
    head.push_str("\n## Boxes now\n");
    if parts.boxes.is_empty() {
        head.push_str("None.\n");
    }
    for line in parts.boxes {
        head.push_str(line);
        head.push('\n');
    }
    let mut tail = String::new();
    if let Some(done) = parts.rerun {
        tail.push_str(
            "\n## Your last turn\nYour last turn stopped when the front restarted. \
             It already did these things:\n",
        );
        if done.is_empty() {
            tail.push_str("Nothing recorded.\n");
        }
        for action in done {
            tail.push_str(&action_line(action));
            tail.push('\n');
        }
        tail.push_str("Do not repeat an action listed above.\n");
    }
    tail.push_str("\n## New\n");
    for said in parts.new {
        tail.push_str(&said_line(said));
        tail.push('\n');
    }
    let earlier = earlier(
        parts.earlier,
        budget.saturating_sub(head.len() + tail.len()),
    );
    head + &earlier + &tail
}

/// The "Earlier in the thread" part within `room` bytes, or nothing when there is nothing
/// earlier.
fn earlier(said: &[Said], room: usize) -> String {
    if said.is_empty() {
        return String::new();
    }
    let header = "\n## Earlier in the thread\n";
    let mut left = room.saturating_sub(header.len() + OMITTED_LINE);
    let mut shown = Vec::new();
    for line in said.iter().rev().map(said_line) {
        if line.len() + 1 > left {
            break;
        }
        left -= line.len() + 1;
        shown.push(line);
    }
    let mut out = header.to_owned();
    let omitted = said.len() - shown.len();
    if omitted > 0 {
        let _ = writeln!(out, "{omitted} earlier messages are not shown.");
    }
    for line in shown.iter().rev() {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// One message as a line, with its speaker.
fn said_line(said: &Said) -> String {
    let text = cut(&said.text, ITEM_MAX, "[the rest is not shown]");
    match &said.speaker {
        Speaker::Person(user) => format!("[<@{user}>] {text}"),
        Speaker::Front => text,
        Speaker::Coordinator => format!("[you] {text}"),
    }
}

/// One journal row as plain facts.
fn action_line(action: &Action) -> String {
    let args = cut(
        &action.args,
        ARGS_MAX,
        "(shortened here; the Box got the full text)",
    );
    let outcome = action.outcome.as_deref().unwrap_or("result unknown");
    format!(
        "Turn {}, <@{}> asked. {} {args}. Result: {outcome}.",
        action.turn, action.requested_by, action.tool
    )
}

/// `text` cut to at most `max` bytes on a character boundary, followed by `marker` when cut.
fn cut(text: &str, max: usize, marker: &str) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let end = (0..=max)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let kept = text.get(..end).unwrap_or_default();
    format!("{kept} {marker}")
}

#[cfg(test)]
mod tests;
