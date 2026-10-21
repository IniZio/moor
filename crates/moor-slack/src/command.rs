//! Decides what a message in a kept thread does, and the front's fixed lines and notices
//! about Boxes.

use moor_core::name::BoxName;

/// Who may use the front and where.
pub(super) struct Allow<'a> {
    /// Channel ids where mentions are answered.
    pub channels: &'a [String],
    /// User ids who may open threads.
    pub users: &'a [String],
}

/// What a message in a thread the front keeps should cause.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplyAction {
    /// Nothing to send; post nothing.
    Ignore,
    /// Queue this text for the thread's coordinator.
    Queue(String),
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
    } else {
        ReplyAction::Queue(text.to_owned())
    }
}

/// The line for a top-level mention from a user off the `users` list.
pub(crate) fn not_allowed(user: &str) -> String {
    format!("<@{user}> is not allowed to start Boxes here; nothing was started.")
}

/// Thread reply after a successful start, with the Box's branch when the reply named one.
pub(super) fn started(name: &BoxName, branch: Option<&str>) -> String {
    let name = name.as_str();
    branch.map_or_else(
        || format!("Started Box `{name}`."),
        |b| format!("Started Box `{name}` on branch `{b}`."),
    )
}

/// Thread reply after a failed start; `message` is the controller's error text.
pub(super) fn failed(name: &BoxName, message: &str) -> String {
    format!("Could not start Box `{}`: {message}", name.as_str())
}

/// A Box event the front queues as a notice for the next turn of the Box's thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxEvent {
    /// The controller started the Box.
    Started,
    /// The controller refused the start.
    NotStarted,
    /// The Box's agent finished its turn.
    Finished,
    /// The Box closed.
    Closed,
}

impl BoxEvent {
    /// The event's part of its notice's key.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::NotStarted => "not_started",
            Self::Finished => "finished",
            Self::Closed => "closed",
        }
    }

    /// The notice for the next turn. It carries no text from a Box or the controller.
    pub(crate) fn notice(self, name: &BoxName) -> String {
        let name = name.as_str();
        let what = match self {
            Self::Started => "started",
            Self::NotStarted => "could not start",
            Self::Finished => "finished its turn",
            Self::Closed => "closed",
        };
        format!("[moor] Box `{name}` {what}.")
    }
}

/// The line the front posts when a Box's agent finishes its turn.
pub(crate) fn finished(name: &BoxName) -> String {
    format!("Box `{}` finished its turn.", name.as_str())
}

/// The line the front posts when a Box closes without a `close_box`.
pub(crate) fn closed(name: &BoxName) -> String {
    format!("Box `{}` closed.", name.as_str())
}

/// Drops leading `<@U...>` / `<@U...|name>` mentions and whitespace.
pub(crate) fn strip_mentions(text: &str) -> &str {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix("<@") {
        let Some((_, tail)) = after.split_once('>') else {
            break;
        };
        rest = tail.trim_start();
    }
    rest
}

#[cfg(test)]
mod tests;
