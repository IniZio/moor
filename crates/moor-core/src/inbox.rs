//! A person's inbox: everything that needed them, newest first, with a read state per item.
//!
//! The controller writes one row when an agent asks, a workflow run waits at an approve step or a
//! run fails. This module is the pure part: what an event adds, when a wait is over and what
//! state an item shows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Why an item is in the inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    /// The agent asked a question or for a permission.
    NeedsInput,
    /// A workflow run waits at an approve step.
    RunWaiting,
    /// A workflow run ended without delivering.
    RunFailed,
}

impl InboxKind {
    /// The kind of the event named `event_kind`; none for an event that adds no item.
    #[must_use]
    pub fn of_event(event_kind: &str) -> Option<Self> {
        match event_kind {
            "needs_input" | "gate_waiting" => Some(Self::NeedsInput),
            "run_waiting" => Some(Self::RunWaiting),
            "run_failed" => Some(Self::RunFailed),
            _ => None,
        }
    }

    /// The kind as stored.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NeedsInput => "needs_input",
            Self::RunWaiting => "run_waiting",
            Self::RunFailed => "run_failed",
        }
    }

    /// The kind as stored, read back; none for a text no kind has.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::of_event(text)
    }
}

/// What an item shows. `Resolved` wins over the read mark: an answered wait needs no one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxState {
    /// Not opened, and its wait (if any) still stands.
    Unread,
    /// Opened, or marked read, and its wait (if any) still stands.
    Read,
    /// Its wait was answered or ended. A failed run has no wait and never gets here.
    Resolved,
}

/// What an event adds to the inbox: the line a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Why it is there.
    pub kind: InboxKind,
    /// What it is about: the Box, or the workflow run.
    pub title: String,
    /// The question asked, or the reason the run failed.
    pub text: String,
}

/// The text of `key` in `payload`, when it says something.
fn field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// What the event of `event_kind` with `payload` adds to the inbox of the Box titled
/// `box_title`; none for an event that adds nothing.
#[must_use]
pub fn entry_of(event_kind: &str, payload: &Value, box_title: &str) -> Option<Entry> {
    let kind = InboxKind::of_event(event_kind)?;
    let run = || {
        let workflow = field(payload, "workflow").unwrap_or("workflow");
        payload
            .get("run")
            .and_then(Value::as_i64)
            .map_or_else(|| workflow.to_owned(), |n| format!("{workflow} (run {n})"))
    };
    Some(match kind {
        InboxKind::NeedsInput => Entry {
            kind,
            title: box_title.to_owned(),
            text: field(payload, "question")
                .unwrap_or("Needs your input")
                .to_owned(),
        },
        InboxKind::RunWaiting => Entry {
            kind,
            title: run(),
            text: field(payload, "question")
                .or_else(|| field(payload, "step"))
                .unwrap_or("Waiting for approval")
                .to_owned(),
        },
        InboxKind::RunFailed => Entry {
            kind,
            title: run(),
            text: field(payload, "reason")
                .unwrap_or("The run did not finish")
                .to_owned(),
        },
    })
}

/// What the controller knows about the wait an item stands for, read when the list is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// The Box is closed.
    pub box_closed: bool,
    /// The id of the Box's newest event that asks or ends a question (see the store), which is
    /// the question it is on now when it is a `needs_input`.
    pub newest_marker: Option<i64>,
    /// The run still waits at the step, since the moment, the item was written for.
    pub run_waits_here: bool,
    /// For an item written for a gate that waits for a person: whether no more gate lines have
    /// been written since the wait began. None for any other item.
    pub gate_waits_here: Option<bool>,
}

/// The number of gate lines written when the gate wait that `payload` tells of began; none for a
/// payload that is not of a gate wait.
#[must_use]
pub fn gate_wait_seq(payload: &Value) -> Option<u64> {
    payload.get("gate")?;
    payload.get("seq")?.as_u64()
}

/// Whether the wait of the item made from event `event` is over.
///
/// A question is over once the Box moved on (the agent works again, however the answer came) or
/// closed; a gate's wait, once the gate moved on or the Box closed. An approve wait is over once its run left that wait, answered, declined or expired.
/// A failed run is a fact, not a wait.
#[must_use]
pub fn is_resolved(kind: InboxKind, event: i64, facts: &Facts) -> bool {
    match kind {
        InboxKind::NeedsInput => {
            facts.box_closed
                || facts
                    .gate_waits_here
                    .map_or(facts.newest_marker != Some(event), |waits| !waits)
        }
        InboxKind::RunWaiting => !facts.run_waits_here,
        InboxKind::RunFailed => false,
    }
}

/// The state an item shows.
#[must_use]
pub const fn state_of(read: bool, resolved: bool) -> InboxState {
    if resolved {
        InboxState::Resolved
    } else if read {
        InboxState::Read
    } else {
        InboxState::Unread
    }
}

/// One item as a front reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItemView {
    /// The item's id.
    pub id: i64,
    /// The Box it is about.
    #[serde(rename = "box")]
    pub box_name: String,
    /// The repo the Box works on.
    pub repo: Option<String>,
    /// Why it is there.
    pub kind: InboxKind,
    /// What it shows.
    pub state: InboxState,
    /// What it is about.
    pub title: String,
    /// The question or reason.
    pub text: String,
    /// RFC 3339 UTC time the event happened.
    pub at: String,
}

/// The inbox, newest first, and how many items are unread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxValue {
    /// The items.
    pub items: Vec<InboxItemView>,
    /// Items that are `Unread`.
    pub unread: u32,
}

impl InboxValue {
    /// The inbox of `items`, counting the unread ones.
    #[must_use]
    pub fn of(items: Vec<InboxItemView>) -> Self {
        let unread = items
            .iter()
            .filter(|i| i.state == InboxState::Unread)
            .count();
        Self {
            unread: u32::try_from(unread).unwrap_or(u32::MAX),
            items,
        }
    }
}

#[cfg(test)]
mod tests;
