//! Decides what a message in a kept thread does, and the front's fixed lines and notices
//! about Boxes.

use moor_core::name::BoxName;
use serde_json::Value;

use crate::escape::{code, escape};

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

/// Thread reply after a successful start, with the Box's branch when the reply named one and a
/// line for each of the start's `notes`.
pub(super) fn started(name: &BoxName, branch: Option<&str>, notes: &[String]) -> String {
    let name = name.as_str();
    let head = branch.map_or_else(
        || format!("Started Box `{name}`."),
        |b| format!("Started Box `{name}` on branch {}.", code(b)),
    );
    notes
        .iter()
        .fold(head, |text, note| format!("{text}\n{}", escape(note)))
}

/// What a start reply says besides its Box: how the work tree started, then each warning. A
/// `show` reply carries neither, so a start settled through `show` posts no notes.
pub(super) fn start_notes(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let tree = value.get("work_tree");
    let state = tree.and_then(|t| t.get("state")).and_then(Value::as_str);
    let reason = tree.and_then(|t| t.get("reason")).and_then(Value::as_str);
    let tree = match (state, reason) {
        (Some("cold_after"), Some(reason)) => Some(format!("Work tree: cold after: {reason}")),
        (Some(state @ ("refreshed" | "cold")), _) => Some(format!("Work tree: {state}")),
        _ => None,
    };
    let warnings = value
        .get("warnings_inline")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|w| format!("Warning: {w}"));
    tree.into_iter().chain(warnings).collect()
}

/// Thread reply after a failed start; `message` is the controller's error text.
pub(super) fn failed(name: &BoxName, message: &str) -> String {
    format!(
        "Could not start Box `{}`: {}",
        name.as_str(),
        escape(message)
    )
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
    /// The Box's agent turn has run past another long-turn threshold.
    LongTurn,
    /// The Box's agent launch failed.
    LaunchFailed,
    /// The Box's agent did not start within the launch limit.
    LaunchTimedOut,
    /// The sleep timer put the Box to sleep.
    AutoSlept,
    /// The Box's agent was started again on its own.
    Relaunched,
    /// The controller refused another automatic agent relaunch.
    RelaunchCapped,
    /// The controller refused a close because the Box holds work that is not pushed.
    Unsaved {
        /// Unpushed commits.
        ahead: u32,
        /// Uncommitted files.
        dirty: u32,
    },
    /// The controller refused a close for another reason.
    CloseRefused,
    /// The Box's agent session ended without a clean exit.
    Stopped(Stop),
}

/// How a Box's agent session ended when it did not exit cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stop {
    /// The exit code, if the controller knew it.
    pub(crate) exit: Option<i32>,
    /// Whether the controller could not save the agent's transcript.
    pub(crate) transcript_lost: bool,
    /// Whether the Box stopped answering, so the controller ended the session itself.
    pub(crate) unreachable: bool,
}

impl Stop {
    /// The line for Box `name`'s thread.
    pub(crate) fn line(self, name: &str) -> String {
        if self.unreachable {
            return format!(
                "`{name}` stopped answering, so its agent session was ended. Reply here and I can \
                 restart it."
            );
        }
        let exit = self
            .exit
            .map_or_else(|| "exit unknown".to_owned(), |n| format!("exit {n}"));
        let lost = if self.transcript_lost {
            ", and its transcript could not be saved"
        } else {
            ""
        };
        format!(
            "`{name}`'s agent stopped unexpectedly ({exit}){lost}. Its work so far is still in the \
             Box. Reply here and I can restart it."
        )
    }
}

impl BoxEvent {
    /// The event's part of its notice's key.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::NotStarted => "not_started",
            Self::Finished => "finished",
            Self::Closed => "closed",
            Self::LongTurn => "long_turn",
            Self::LaunchFailed => "launch_failed",
            Self::LaunchTimedOut => "launch_timed_out",
            Self::AutoSlept => "auto_slept",
            Self::Relaunched => "relaunched",
            Self::RelaunchCapped => "relaunch_capped",
            Self::Unsaved { .. } => "unsaved",
            Self::CloseRefused => "close_refused",
            Self::Stopped(_) => "stopped",
        }
    }

    /// The notice for the next turn. It carries no text from a Box or the controller.
    pub(crate) fn notice(self, name: &BoxName) -> String {
        let name = name.as_str();
        let what = match self {
            Self::Unsaved { ahead, dirty } => {
                return format!(
                    "[moor] Box `{name}` was not closed: it holds {}, which closing would \
                     discard. The thread was asked to confirm. Call close_box with discard only \
                     after the person who asked for the Box confirms in the thread.",
                    unsaved_work(ahead, dirty)
                )
            }
            Self::Stopped(stop) => return format!("[moor] {}", stop.line(name)),
            Self::CloseRefused => "was not closed; the thread was told why",
            Self::Started => "started",
            Self::NotStarted => "could not start",
            Self::Finished => "finished its turn",
            Self::Closed => "closed",
            Self::LongTurn => "has been working on one turn for a long time",
            Self::LaunchFailed => "could not start its agent",
            Self::LaunchTimedOut => "did not start its agent in time",
            Self::AutoSlept => "went to sleep to save resources",
            Self::Relaunched => "had its agent started again",
            Self::RelaunchCapped => "was not relaunched, the automatic relaunch cap was reached",
        };
        format!("[moor] Box `{name}` {what}.")
    }
}

/// Unsaved work in Slack terms, such as `2 unpushed commits and 1 uncommitted file`.
pub(crate) fn unsaved_work(ahead: u32, dirty: u32) -> String {
    let count = |n: u32, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    match (ahead, dirty) {
        (0, dirty) => count(dirty, "uncommitted file"),
        (ahead, 0) => count(ahead, "unpushed commit"),
        (ahead, dirty) => format!(
            "{} and {}",
            count(ahead, "unpushed commit"),
            count(dirty, "uncommitted file")
        ),
    }
}

/// The report that starts a coordinator turn when Box `name`'s agent finishes its turn, with
/// the agent's last message in a Box quote when it left one.
pub(crate) fn report(name: &BoxName, said: Option<&str>) -> String {
    let head = BoxEvent::Finished.notice(name);
    let Some(said) = said.filter(|s| !s.trim().is_empty()) else {
        return head;
    };
    let mut text = format!("{head} Its last message, in a Box quote:");
    for line in said.lines() {
        text.push_str("\n> ");
        text.push_str(line);
    }
    text
}

/// The line the front posts when a Box's agent finishes its turn.
pub(crate) fn finished(name: &BoxName) -> String {
    format!("Box `{}` finished its turn.", name.as_str())
}

/// The line the front posts when a Box closes without a `close_box`.
pub(crate) fn closed(name: &BoxName) -> String {
    format!("Box `{}` closed.", name.as_str())
}

/// The positive number a message consists of, ignoring user mentions.
pub(crate) fn bare_number(text: &str) -> Option<u32> {
    let mut words = text.split_whitespace().filter(|w| !w.starts_with("<@"));
    let n = words.next()?.parse::<u32>().ok()?;
    (words.next().is_none() && n > 0).then_some(n)
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
