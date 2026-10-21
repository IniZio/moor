//! Parses app mentions and thread messages and decides what each one does.

use moor_core::name::{BoxName, NameError};

use crate::db::{BoxState, SlackBox};

/// Reply for text that is not a well-formed `start` command.
const USAGE: &str = "Usage: `@moor start <repo> <goal>`";

/// Who may start Boxes and where.
pub(super) struct Allow<'a> {
    /// Channel ids where mentions are answered.
    pub channels: &'a [String],
    /// User ids who may start Boxes.
    pub users: &'a [String],
}

/// What one app mention should cause.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Not ours to answer (channel not allowed, or text addressed to nobody we parse); post nothing.
    Ignore,
    /// Post this text in the thread and start nothing.
    Reply(String),
    /// Start this Box, then bind the thread to it.
    Start {
        /// Box name derived from the thread.
        name: BoxName,
        /// Repo the Box works on.
        repo: String,
        /// What the Box should do.
        goal: String,
    },
}

/// What a message in a thread the front keeps should cause.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplyAction {
    /// Nothing to send; post nothing.
    Ignore,
    /// Queue this text for the thread's coordinator.
    Queue(String),
    /// Close the thread's open Box.
    Close,
    /// Post this text in the thread and change nothing.
    Refuse(String),
}

/// Decides what a message in a kept thread does; only the thread's `starter` may direct it.
pub(crate) fn reply_action(starter: &str, user: &str, text: &str) -> ReplyAction {
    if user != starter {
        return ReplyAction::Refuse(format!("Only <@{starter}> can direct this thread."));
    }
    let text = strip_mentions(text).trim();
    if text.is_empty() {
        ReplyAction::Ignore
    } else if text.eq_ignore_ascii_case("close") {
        ReplyAction::Close
    } else {
        ReplyAction::Queue(text.to_owned())
    }
}

/// The Box a `close` from `user` closes: the thread's one open Box, when `user` asked for it
/// and it is bound. Otherwise the line that says why nothing closes.
pub(crate) fn close_target<'a>(boxes: &'a [SlackBox], user: &str) -> Result<&'a SlackBox, String> {
    let open: Vec<_> = boxes
        .iter()
        .filter(|b| !matches!(b.state, BoxState::Gone { .. }))
        .collect();
    let target = match open.as_slice() {
        [] => return Err("This thread has no open Box.".to_owned()),
        [one] => *one,
        many => {
            return Err(format!(
                "This thread has {} open Boxes. Ask the coordinator which to close.",
                many.len()
            ))
        }
    };
    let name = target.box_name.as_str();
    if target.requested_by != user {
        let by = &target.requested_by;
        return Err(format!("Only <@{by}> can close Box `{name}`."));
    }
    match target.state {
        BoxState::Starting => Err(format!(
            "Box `{name}` is still starting. Send close again once it has started."
        )),
        BoxState::Closing { .. } => Err(format!("Box `{name}` is already closing.")),
        BoxState::Bound { .. } | BoxState::Gone { .. } => Ok(target),
    }
}

/// True when `text` is a well-formed `start` command.
pub(crate) fn is_start(text: &str) -> bool {
    parse_start(strip_mentions(text)).is_some()
}

/// Decides what one app mention does. `thread` is the thread root ts (`thread_ts`, else the
/// message ts); `existing` is that thread's latest Box in `slack.db`, if it has one.
pub(super) fn decide(
    allow: &Allow<'_>,
    channel: &str,
    user: &str,
    text: &str,
    thread: &str,
    existing: Option<&SlackBox>,
) -> Action {
    if !allow.channels.iter().any(|c| c == channel) {
        return Action::Ignore;
    }
    if !allow.users.iter().any(|u| u == user) {
        return Action::Reply(format!(
            "<@{user}> is not allowed to start Boxes here; nothing was started."
        ));
    }
    let Some((repo, goal)) = parse_start(strip_mentions(text)) else {
        return Action::Reply(USAGE.to_owned());
    };
    if let Some(row) = existing {
        let name = row.box_name.as_str();
        return Action::Reply(match row.state {
            BoxState::Starting => {
                format!("Box `{name}` is still starting; nothing new was started.")
            }
            BoxState::Bound { .. } | BoxState::Closing { .. } => {
                format!("This thread already runs Box `{name}`; nothing was started.")
            }
            BoxState::Gone { .. } => {
                format!("Box `{name}` of this thread is closed; nothing was started.")
            }
        });
    }
    match box_name(thread) {
        Ok(name) => Action::Start {
            name,
            repo: repo.to_owned(),
            goal: goal.to_owned(),
        },
        Err(e) => Action::Reply(e.to_string()),
    }
}

/// Box name for a thread root ts: "slack-" + ts with '.' replaced by '-'.
///
/// # Errors
/// Returns the name error when the ts yields an invalid Box name.
pub(super) fn box_name(thread: &str) -> Result<BoxName, NameError> {
    BoxName::parse_allow_bare(&format!("slack-{}", thread.replace('.', "-")))
}

/// Thread reply after a successful start.
pub(super) fn started(name: &BoxName) -> String {
    format!("Started Box `{}`.", name.as_str())
}

/// Thread reply after a failed start; `message` is the controller's error text.
pub(super) fn failed(name: &BoxName, message: &str) -> String {
    format!("Could not start Box `{}`: {message}", name.as_str())
}

/// Drops leading `<@U...>` / `<@U...|name>` mentions and whitespace.
fn strip_mentions(text: &str) -> &str {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix("<@") {
        let Some((_, tail)) = after.split_once('>') else {
            break;
        };
        rest = tail.trim_start();
    }
    rest
}

/// Splits `start <repo> <goal...>` into repo and goal; `None` when malformed.
fn parse_start(text: &str) -> Option<(&str, &str)> {
    let (verb, rest) = text.split_once(char::is_whitespace)?;
    if !verb.eq_ignore_ascii_case("start") {
        return None;
    }
    let (repo, goal) = rest.trim_start().split_once(char::is_whitespace)?;
    let goal = goal.trim();
    (!repo.is_empty() && !goal.is_empty()).then_some((repo, goal))
}

#[cfg(test)]
mod tests;
