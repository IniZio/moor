//! A transcript's lines in the timeline, among the Box's own events.

use std::collections::HashSet;

use serde_json::Value;

use super::{scrub, Line, What};
use crate::progress::timeline::{Class, TimelineEntry};
use crate::view::EventView;

/// A Box's saved transcript, as the timeline reads it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Transcript<'a> {
    /// Its lines, oldest first.
    pub lines: &'a [Line],
    /// When it was taken, in milliseconds: the agent's one-line steps and messages until then
    /// are in its lines.
    pub saved_ms: i64,
}

impl Transcript<'_> {
    /// Whether event `e` at `ms` says again what a line already says: a one-line step or a
    /// message between the first line kept and the save. Later ones stay, the live view of a turn
    /// not yet saved, and so do earlier ones, whose lines the cap dropped.
    pub(crate) fn covers(&self, e: &EventView, ms: i64) -> bool {
        self.lines
            .first()
            .is_some_and(|first| first.ms <= ms && ms <= self.saved_ms)
            && matches!(e.kind.as_str(), "agent_doing" | "agent_said")
    }
}

impl Line {
    /// The timeline entry of this line.
    pub(crate) fn entry(&self) -> TimelineEntry {
        let (kind, class, text, tool) = match &self.what {
            What::Said { text } => ("said", Class::Message, text.clone(), None),
            What::Typed { text } => ("typed", Class::You, text.clone(), None),
            What::Tool(t) => (
                "tool",
                if t.failed { Class::Error } else { Class::Step },
                t.title.clone(),
                Some(t.clone()),
            ),
        };
        TimelineEntry {
            id: 0,
            at: self.at.clone(),
            kind: kind.to_owned(),
            class,
            text,
            artifact: None,
            step: None,
            quote: None,
            receipt: None,
            key: Some(self.id.clone()),
            tool,
        }
    }
}

/// The words moor sent the agent, as the Box's `steered` events record them.
pub(crate) struct Sent(HashSet<String>);

impl Sent {
    /// The words of the steered events among `events`, and of a menu choice's typed text.
    pub(crate) fn of<'e>(events: impl Iterator<Item = &'e EventView>) -> Self {
        let mut words = HashSet::new();
        for e in events.filter(|e| e.kind == "steered") {
            let Some(message) = e.payload.get("message").and_then(Value::as_str) else {
                continue;
            };
            words.insert(norm(message));
            if let Some((n, typed)) = message.split_once(": ") {
                if n.parse::<u8>().is_ok() {
                    words.insert(norm(typed));
                }
            }
        }
        Self(words)
    }

    /// Whether `line` is typed words moor sent itself, so a steer already shows them.
    pub(crate) fn has_sent(&self, line: &Line) -> bool {
        matches!(&line.what, What::Typed { text } if self.0.contains(&norm(text)))
    }
}

/// `text` as lines keep it, with each run of white space as one space, so a paste and a steer
/// compare equal.
fn norm(text: &str) -> String {
    scrub(text).split_whitespace().collect::<Vec<_>>().join(" ")
}
