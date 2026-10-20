//! Parses app mentions and decides what each one does.

use moor_core::name::{BoxName, NameError};

use crate::db::{SlackThread, ThreadState};

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

/// What a plain thread reply in a bound thread should cause.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplyAction {
    /// Nothing to send; post nothing.
    Ignore,
    /// Steer the Box with this text.
    Steer(String),
    /// Close the Box.
    Close,
    /// Post this text in the thread and change nothing.
    Refuse(String),
}

/// Decides what a reply in a bound thread does; only the Box `owner` may steer or close.
pub(crate) fn reply_action(owner: &str, user: &str, text: &str) -> ReplyAction {
    if user != owner {
        return ReplyAction::Refuse(format!("Only <@{owner}> can steer this Box."));
    }
    let text = strip_mentions(text).trim();
    if text.is_empty() {
        ReplyAction::Ignore
    } else if text.eq_ignore_ascii_case("close") {
        ReplyAction::Close
    } else {
        ReplyAction::Steer(text.to_owned())
    }
}

/// Decides what one app mention does. `thread` is the thread root ts (`thread_ts`, else the
/// message ts); `existing` is that thread's row in `slack.db`, if it has one.
pub(super) fn decide(
    allow: &Allow<'_>,
    channel: &str,
    user: &str,
    text: &str,
    thread: &str,
    existing: Option<&SlackThread>,
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
            ThreadState::Starting => {
                format!("Box `{name}` is still starting; nothing new was started.")
            }
            ThreadState::Bound { .. } => {
                format!("This thread already runs Box `{name}`; nothing was started.")
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
