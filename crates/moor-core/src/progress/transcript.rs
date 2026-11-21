//! What the agent did, read from Claude's own session transcript.
//!
//! Each tool call keeps its input and output, and each message to the person its text. Every
//! text is scrubbed of anything shaped like a credential, then cut to size, before it is kept.

mod entry;
mod scrub;
mod tool;
mod typed;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use entry::Transcript;
pub(crate) use entry::{Refused, Sent};
pub use scrub::scrub;

/// The most lines a transcript keeps; the oldest go first.
pub const MAX_LINES: usize = 600;

/// The most bytes of input, diff and output all lines keep; older lines lose theirs first.
pub const BUDGET: usize = 512 * 1024;

/// The most bytes a transcript's lines take as JSON: [`MAX_LINES`] titles and short messages, and
/// [`BUDGET`] of bodies, well inside it.
pub const STORED_MAX: u64 = 4 * 1024 * 1024;

/// A transcript record longer than this is skipped unread.
pub const RECORD_MAX: usize = 4 * 1024 * 1024;

/// The most characters one input, diff or output keeps.
const BODY_MAX: usize = 6000;

/// The most characters one message keeps.
const SAID_MAX: usize = 16_000;

/// The most characters a message past [`BUDGET`] keeps.
const SAID_SHORT: usize = 1000;

/// The most characters a title keeps.
const TITLE_MAX: usize = 200;

/// One thing the agent did or said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// The tool call's id, or the message's id and block number: unique in the session.
    pub id: String,
    /// When, as the transcript wrote it.
    pub at: String,
    /// The same, in milliseconds since the epoch.
    pub ms: i64,
    /// What it was.
    #[serde(flatten)]
    pub what: What,
}

/// What a line holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    /// The agent wrote to the person, in markdown.
    Said {
        /// The message.
        text: String,
    },
    /// The agent called a tool.
    Tool(Tool),
    /// The person typed this into the agent's own terminal.
    Typed {
        /// What they typed.
        text: String,
    },
}

/// One tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// Claude's name for the tool.
    pub name: String,
    /// One line, such as `Edited src/login.tsx` or `Ran npm test (exit 1)`.
    pub title: String,
    /// For a routine call, what it folds with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Group>,
    /// The call failed, exited non-zero or was refused: never folded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// The call was a report moor recorded it did not accept: normal flow, so not `failed` and
    /// not red.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refused: bool,
    /// What the call was given: a command, a search, a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Body>,
    /// For an edit, the change as diff lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<Body>,
    /// What the call gave back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Body>,
}

/// What a routine call folds with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// It read a file, a folder or a page.
    Read,
    /// It searched.
    Search,
    /// It updated the agent's own plan.
    Plan,
}

/// A text, cut to size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    /// The text kept.
    pub text: String,
    /// Part of the text was cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

impl Body {
    /// `text`, scrubbed, with its start kept when it is too long.
    fn head(text: &str) -> Self {
        cut(&scrub(text), BODY_MAX, false)
    }

    /// `text`, scrubbed, with its start and its end kept when it is too long: a command's error
    /// is at its end.
    fn ends(text: &str) -> Self {
        cut(&scrub(text), BODY_MAX, true)
    }

    /// The bytes it holds.
    const fn size(&self) -> usize {
        self.text.len()
    }
}

/// `text` cut to `max` characters: its start, or with `ends` a third from its start and the rest
/// from its end.
fn cut(text: &str, max: usize, ends: bool) -> Body {
    let count = text.chars().count();
    if count <= max {
        return Body {
            text: text.to_owned(),
            cut: false,
        };
    }
    let at = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    let keep = if ends { max / 3 } else { max };
    let head = text.get(..at(keep)).unwrap_or_default();
    let tail = if ends {
        text.get(at(count - (max - keep))..).unwrap_or_default()
    } else {
        ""
    };
    let gone = count - keep - tail.chars().count();
    Body {
        text: format!("{head}\n… {gone} characters cut …\n{tail}"),
        cut: true,
    }
}

/// Reads a session's records in order, one JSON object per line, into its lines.
#[derive(Debug, Default)]
pub struct Reader {
    /// The lines so far, in the order the records gave them.
    lines: Vec<Line>,
    /// Where each tool call still without a result sits in `lines`, by its id.
    calls: HashMap<String, usize>,
    /// Every id read, so a record repeated in another session file is kept once.
    seen: HashSet<String>,
}

impl Reader {
    /// Reads one record; one too long, not JSON, of another kind or from a subagent is skipped.
    pub fn read(&mut self, record: &str) {
        if record.len() > RECORD_MAX {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(record) else {
            return;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if v.get("type").and_then(Value::as_str) == Some("user") {
            if let Some(text) = typed::prompt(&v) {
                return self.typed(&v, &text);
            }
        }
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => self.assistant(&v, blocks),
            Some("user") => self.results(&v, blocks),
            _ => {}
        }
    }

    /// The line of a user record the person typed: its words, scrubbed and cut.
    fn typed(&mut self, v: &Value, text: &str) {
        let id = text_of(v, "uuid");
        if let (Some((at, ms)), false) = (when(v), id.is_empty()) {
            let text = kept(text);
            self.push(Line {
                id: id.to_owned(),
                at,
                ms,
                what: What::Typed { text },
            });
        }
    }

    /// The lines of an assistant record: its messages and tool calls. Thinking is not kept.
    fn assistant(&mut self, v: &Value, blocks: &[Value]) {
        let Some((at, ms)) = when(v) else {
            return;
        };
        let cwd = text_of(v, "cwd");
        let uuid = text_of(v, "uuid");
        for (n, b) in blocks.iter().enumerate() {
            let what = match text_of(b, "type") {
                "text" => said(text_of(b, "text")).map(|w| (format!("{uuid}:{n}"), w)),
                "tool_use" => Some((
                    text_of(b, "id").to_owned(),
                    What::Tool(tool::called(
                        text_of(b, "name"),
                        b.get("input").unwrap_or(&Value::Null),
                        cwd,
                    )),
                )),
                _ => None,
            };
            if let Some((id, what)) = what.filter(|(id, _)| !id.is_empty()) {
                self.push(Line {
                    id,
                    at: at.clone(),
                    ms,
                    what,
                });
            }
        }
    }

    /// Keeps `line` unless its id was read before.
    fn push(&mut self, line: Line) {
        if !self.seen.insert(line.id.clone()) {
            return;
        }
        if matches!(line.what, What::Tool(_)) {
            self.calls.insert(line.id.clone(), self.lines.len());
        }
        self.lines.push(line);
    }

    /// Gives each tool result in a user record to its call.
    fn results(&mut self, v: &Value, blocks: &[Value]) {
        let detail = v.get("toolUseResult");
        for b in blocks
            .iter()
            .filter(|b| text_of(b, "type") == "tool_result")
        {
            let Some(at) = self.calls.remove(text_of(b, "tool_use_id")) else {
                continue;
            };
            if let Some(Line {
                what: What::Tool(t),
                ..
            }) = self.lines.get_mut(at)
            {
                tool::finished(t, b, detail);
            }
        }
    }

    /// The lines, oldest first, at most [`MAX_LINES`], within [`BUDGET`].
    #[must_use]
    pub fn finish(self) -> Vec<Line> {
        let mut lines = self.lines;
        lines.sort_by_key(|l| l.ms);
        let old = lines.len().saturating_sub(MAX_LINES);
        lines.drain(..old);
        fit(&mut lines);
        lines
    }
}

/// The lines of `records`, as [`Reader`] reads them.
#[must_use]
pub fn extract<'a>(records: impl IntoIterator<Item = &'a str>) -> Vec<Line> {
    let mut reader = Reader::default();
    records.into_iter().for_each(|r| reader.read(r));
    reader.finish()
}

/// Keeps the newest lines whole within [`BUDGET`]: older tool calls lose their bodies, and older
/// messages are cut short.
fn fit(lines: &mut [Line]) {
    let mut used = 0;
    for line in lines.iter_mut().rev() {
        used += match &line.what {
            What::Said { text } | What::Typed { text } => text.len(),
            What::Tool(t) => [&t.input, &t.diff, &t.output]
                .iter()
                .filter_map(|b| b.as_ref().map(Body::size))
                .sum::<usize>(),
        };
        if used <= BUDGET {
            continue;
        }
        match &mut line.what {
            What::Said { text } | What::Typed { text } => *text = cut(text, SAID_SHORT, false).text,
            What::Tool(t) => (t.input, t.diff, t.output) = (None, None, None),
        }
    }
}

/// A message, scrubbed and cut; none when blank.
fn said(text: &str) -> Option<What> {
    let text = text.trim();
    (!text.is_empty()).then(|| What::Said { text: kept(text) })
}

/// `text` as a line keeps it: scrubbed, then cut to [`SAID_MAX`].
fn kept(text: &str) -> String {
    cut(&scrub(text), SAID_MAX, false).text
}

/// The record's time, as written and in milliseconds.
fn when(v: &Value) -> Option<(String, i64)> {
    let at = text_of(v, "timestamp");
    let ms = at.parse::<jiff::Timestamp>().ok()?.as_millisecond();
    Some((at.to_owned(), ms))
}

/// `v`'s text field `key`, empty when absent.
fn text_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests;
