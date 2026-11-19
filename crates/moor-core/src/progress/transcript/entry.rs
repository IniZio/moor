//! A transcript's lines in the timeline, among the Box's own events.

use super::{Line, What};
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
            receipt: None,
            key: Some(self.id.clone()),
            tool,
        }
    }
}
