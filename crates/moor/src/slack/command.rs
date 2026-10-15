//! Parses app mentions and decides what each one does.

use moor_core::name::{NameError, TaskName};

/// Reply for text that is not a well-formed `start` command.
const USAGE: &str = "Usage: `@moor start <repo> <goal>`";

/// Who may start Tasks and where.
pub(super) struct Allow<'a> {
    /// Channel ids where mentions are answered.
    pub channels: &'a [String],
    /// User ids who may start Tasks.
    pub users: &'a [String],
}

/// What one app mention should cause.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Not ours to answer (channel not allowed, or text addressed to nobody we parse); post nothing.
    Ignore,
    /// Post this text in the thread and start nothing.
    Reply(String),
    /// Start this Task, then bind the thread to it.
    Start {
        /// Task name derived from the thread.
        name: TaskName,
        /// Repo the Task works on.
        repo: String,
        /// What the Task should do.
        goal: String,
    },
}

/// What a plain thread reply in a bound thread should cause.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplyAction {
    /// Nothing to send; post nothing.
    Ignore,
    /// Steer the Task with this text.
    Steer(String),
    /// Close the Task.
    Close,
    /// Post this text in the thread and change nothing.
    Refuse(String),
}

/// Decides what a reply in a bound thread does; only the Task `owner` may steer or close.
pub(crate) fn reply_action(owner: &str, user: &str, text: &str) -> ReplyAction {
    if user != owner {
        return ReplyAction::Refuse(format!("Only <@{owner}> can steer this Task."));
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
/// message ts); `bound` is the Task already bound to that thread.
pub(super) fn decide(
    allow: &Allow<'_>,
    channel: &str,
    user: &str,
    text: &str,
    thread: &str,
    bound: Option<&TaskName>,
) -> Action {
    if !allow.channels.iter().any(|c| c == channel) {
        return Action::Ignore;
    }
    if !allow.users.iter().any(|u| u == user) {
        return Action::Reply(format!(
            "<@{user}> is not allowed to start Tasks here; nothing was started."
        ));
    }
    let Some((repo, goal)) = parse_start(strip_mentions(text)) else {
        return Action::Reply(USAGE.to_owned());
    };
    if let Some(name) = bound {
        return Action::Reply(format!(
            "This thread already runs Task `{}`; nothing was started.",
            name.as_str()
        ));
    }
    match task_name(thread) {
        Ok(name) => Action::Start {
            name,
            repo: repo.to_owned(),
            goal: goal.to_owned(),
        },
        Err(e) => Action::Reply(e.to_string()),
    }
}

/// Task name for a thread root ts: "slack-" + ts with '.' replaced by '-'.
///
/// # Errors
/// Returns the name error when the ts yields an invalid Task name.
pub(super) fn task_name(thread: &str) -> Result<TaskName, NameError> {
    TaskName::parse(&format!("slack-{}", thread.replace('.', "-")))
}

/// Thread reply after a successful start.
pub(super) fn started(name: &TaskName) -> String {
    format!("Started Task `{}`.", name.as_str())
}

/// Thread reply after a failed start; `message` is the controller's error text.
pub(super) fn failed(name: &TaskName, message: &str) -> String {
    format!("Could not start Task `{}`: {message}", name.as_str())
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
