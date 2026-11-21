//! A transcript's lines in the timeline, among the Box's own events.

use std::collections::HashSet;

use serde_json::Value;

use super::{scrub, Body, Line, Tool, What};
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

/// What a report moor did not accept reads as.
const REFUSED: &str = "moor did not accept the report";

/// The replies of the reports moor recorded it refused, from the Box's `report_refused` events,
/// and those of them a transcript line shows. A failed call reads as a refusal only through
/// such a record, so a report that never reached moor stays an error, whatever the agent's tool
/// is called.
pub(crate) struct Refused {
    /// Every recorded reply.
    reasons: HashSet<String>,
    /// The replies a transcript line shows.
    shown: HashSet<String>,
}

impl Refused {
    /// The refusals recorded among `events`, matched against `lines`.
    pub(crate) fn of<'e>(events: impl Iterator<Item = &'e EventView>, lines: &[Line]) -> Self {
        let reasons: HashSet<String> = events
            .filter(|e| e.kind == "report_refused")
            .filter_map(|e| e.payload.get("reason").and_then(Value::as_str).map(norm))
            .collect();
        let shown = lines
            .iter()
            .filter_map(answered)
            .filter(|r| reasons.contains(r))
            .collect();
        Self { reasons, shown }
    }

    /// The timeline entry of `line`; a call whose reply moor recorded as a refusal is the plain
    /// item, not an error.
    pub(crate) fn entry(&self, line: &Line) -> TimelineEntry {
        let mut entry = line.entry();
        if !answered(line).is_some_and(|r| self.reasons.contains(&r)) {
            return entry;
        }
        if let Some(t) = entry.tool.as_mut() {
            (t.failed, t.refused) = (false, true);
            REFUSED.clone_into(&mut t.title);
        }
        (entry.class, entry.text) = (Class::Step, REFUSED.to_owned());
        entry
    }

    /// The entry of the `report_refused` event `e` when no transcript line shows it, as for an
    /// agent whose transcript moor does not read; none when a line does.
    pub(crate) fn unshown(&self, e: &EventView) -> Option<TimelineEntry> {
        let reason = e.payload.get("reason").and_then(Value::as_str)?;
        if self.shown.contains(&norm(reason)) {
            return None;
        }
        let tool = Tool {
            name: "report_step".to_owned(),
            title: REFUSED.to_owned(),
            group: None,
            failed: false,
            refused: true,
            input: None,
            diff: None,
            output: Some(Body::ends(reason)),
        };
        Some(TimelineEntry {
            id: e.id,
            at: e.ts.clone(),
            kind: "tool".to_owned(),
            class: Class::Step,
            text: REFUSED.to_owned(),
            artifact: None,
            step: None,
            quote: None,
            receipt: None,
            key: None,
            tool: Some(tool),
        })
    }
}

/// The reply of a call that failed, or was read as refused before refusals were recorded.
fn answered(line: &Line) -> Option<String> {
    match &line.what {
        What::Tool(t) if t.failed || t.refused => t.output.as_ref().map(|b| norm(&b.text)),
        _ => None,
    }
}

/// `text` as lines keep it, with each run of white space as one space, so a paste and a steer
/// compare equal.
fn norm(text: &str) -> String {
    scrub(text).split_whitespace().collect::<Vec<_>>().join(" ")
}
