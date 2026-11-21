//! One tool call in words: its title, what it folds with, its input, diff and output.

use std::fmt::Write as _;

use serde_json::Value;

use super::{text_of, Body, Group, Tool};

/// The most characters of a command a title shows.
const COMMAND_MAX: usize = 80;

/// What a call shows: its title, its fold, its input and its diff.
type Shape = (String, Option<Group>, Option<Body>, Option<Body>);

/// The call of tool `name` with `input`, made in `cwd`, before its result is known.
pub(super) fn called(name: &str, input: &Value, cwd: &str) -> Tool {
    let (title, group, input_body, diff) = looked(name, input, cwd)
        .or_else(|| changed(name, input, cwd))
        .unwrap_or_else(|| other(name, input));
    Tool {
        name: name.to_owned(),
        title: clip(&super::scrub(&title), super::TITLE_MAX),
        group,
        failed: false,
        refused: false,
        input: input_body,
        diff,
        output: None,
    }
}

/// A call that only looks: it reads, lists, searches or updates the plan, and folds.
fn looked(name: &str, input: &Value, cwd: &str) -> Option<Shape> {
    let s = |key: &str| text_of(input, key);
    Some(match name {
        "Read" => (
            format!("Read {}", path(input, cwd)),
            Some(Group::Read),
            None,
            None,
        ),
        "LS" => (
            format!("Listed {}", short(s("path"), cwd)),
            Some(Group::Read),
            None,
            None,
        ),
        "WebFetch" => (
            format!("Fetched {}", s("url")),
            Some(Group::Read),
            None,
            None,
        ),
        "Grep" | "Glob" => (
            format!("Searched for {}", s("pattern")),
            Some(Group::Search),
            Some(Body::head(&search(input, cwd))),
            None,
        ),
        "WebSearch" => (
            format!("Searched the web for {}", s("query")),
            Some(Group::Search),
            None,
            None,
        ),
        "TodoWrite" => (
            "Updated the plan".to_owned(),
            Some(Group::Plan),
            Some(Body::head(&todos(input))),
            None,
        ),
        _ => return None,
    })
}

/// A call that changes something or runs: an edit, a write, a command, a subagent.
fn changed(name: &str, input: &Value, cwd: &str) -> Option<Shape> {
    let s = |key: &str| text_of(input, key);
    Some(match name {
        "Edit" | "MultiEdit" | "NotebookEdit" => (
            format!("Edited {}", path(input, cwd)),
            None,
            None,
            Some(Body::head(&replaced(input))),
        ),
        "Write" => (
            format!("Wrote {}", path(input, cwd)),
            None,
            None,
            Some(Body::head(&added(s("content")))),
        ),
        "Bash" => (
            format!("Ran {}", clip(s("command"), COMMAND_MAX)),
            None,
            Some(Body::head(s("command"))),
            None,
        ),
        "Agent" => (
            format!("Ran a subagent: {}", clip(s("description"), COMMAND_MAX)),
            None,
            Some(Body::head(s("prompt"))),
            None,
        ),
        _ => return None,
    })
}

/// Any other call, with its input as JSON.
fn other(name: &str, input: &Value) -> Shape {
    let json = serde_json::to_string_pretty(input).unwrap_or_default();
    (
        format!("Used {}", short_name(name)),
        None,
        Some(Body::head(&json)),
        None,
    )
}

/// The file a call names, under `cwd`.
fn path<'a>(input: &'a Value, cwd: &str) -> &'a str {
    let p = Some(text_of(input, "file_path"))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| text_of(input, "notebook_path"));
    short(p, cwd)
}

/// Gives `t` its result: the `tool_result` block and the record's own `detail`.
pub(super) fn finished(t: &mut Tool, block: &Value, detail: Option<&Value>) {
    let text = result_text(block.get("content"));
    if let Some(patch) = detail.and_then(patch) {
        t.diff = Some(Body::head(&patch));
    }
    t.failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
    if t.failed {
        let _ = write!(t.title, " ({})", why(&text));
    }
    let edit = t.diff.is_some() && !t.failed;
    if !edit && !text.trim().is_empty() {
        t.output = Some(Body::ends(&text));
    }
}

/// Why a call failed, in a word or two: its exit code, a refusal, or that it failed.
fn why(text: &str) -> String {
    let code = text
        .strip_prefix("Exit code ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse::<i64>().ok());
    if let Some(code) = code {
        return format!("exit {code}");
    }
    let lower = text.to_lowercase();
    let refused = [
        "doesn't want to proceed",
        "permission to use",
        "was denied",
        "blocked",
    ]
    .iter()
    .any(|w| lower.contains(w));
    if refused { "refused" } else { "failed" }.to_owned()
}

/// A tool result's text: a string, or its text blocks joined.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| match text_of(b, "type") {
                "text" => text_of(b, "text").to_owned(),
                "image" => "[image]".to_owned(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// An edit's hunks as diff lines, from Claude's `structuredPatch`; none when it has none.
fn patch(detail: &Value) -> Option<String> {
    let hunks = detail.get("structuredPatch")?.as_array()?;
    let mut out = String::new();
    for h in hunks {
        let n = |k: &str| h.get(k).and_then(Value::as_i64).unwrap_or_default();
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            n("oldStart"),
            n("oldLines"),
            n("newStart"),
            n("newLines")
        );
        for line in h
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.push_str(line.as_str().unwrap_or_default());
            out.push('\n');
        }
    }
    (!out.is_empty()).then_some(out)
}

/// An edit's old and new text.
fn pair(e: &Value) -> (&str, &str) {
    (text_of(e, "old_string"), text_of(e, "new_string"))
}

/// An edit's own words as diff lines, for a call that has no result yet.
fn replaced(input: &Value) -> String {
    let pairs: Vec<(&str, &str)> = input.get("edits").and_then(Value::as_array).map_or_else(
        || vec![pair(input)],
        |edits| edits.iter().map(pair).collect(),
    );
    let mut out = String::new();
    for (old, new) in pairs {
        old.lines().for_each(|l| {
            let _ = writeln!(out, "-{l}");
        });
        out.push_str(&added(new));
    }
    if out.is_empty() {
        out = added(text_of(input, "new_source"));
    }
    out
}

/// Every line of `text` as an added diff line.
fn added(text: &str) -> String {
    text.lines().fold(String::new(), |mut out, l| {
        let _ = writeln!(out, "+{l}");
        out
    })
}

/// A search's pattern and where it looked.
fn search(input: &Value, cwd: &str) -> String {
    let mut out = text_of(input, "pattern").to_owned();
    for (key, word) in [("path", "in"), ("glob", "files"), ("type", "type")] {
        let v = text_of(input, key);
        if !v.is_empty() {
            let _ = write!(out, "\n{word} {}", short(v, cwd));
        }
    }
    out
}

/// The plan's items, one per line, with their state.
fn todos(input: &Value) -> String {
    input
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| {
            let mark = match text_of(t, "status") {
                "completed" => "✓",
                "in_progress" => "●",
                _ => "○",
            };
            format!("{mark} {}", text_of(t, "content"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `path` under `cwd`, else as it is.
fn short<'a>(path: &'a str, cwd: &str) -> &'a str {
    if cwd.is_empty() {
        return path;
    }
    path.strip_prefix(cwd)
        .and_then(|p| p.strip_prefix('/'))
        .filter(|p| !p.is_empty())
        .unwrap_or(path)
}

/// An MCP tool's own name, with its server: `mcp__moor__start_box` reads `start_box (moor)`.
fn short_name(name: &str) -> String {
    let mut parts = name.splitn(3, "__");
    match (parts.next(), parts.next(), parts.next()) {
        (Some("mcp"), Some(server), Some(tool)) => format!("{tool} ({server})"),
        _ => name.to_owned(),
    }
}

/// The first line of `text`, at most `max` characters.
fn clip(text: &str, max: usize) -> String {
    let first = text.trim().lines().next().unwrap_or_default();
    let more = text.trim().lines().nth(1).is_some();
    if first.chars().count() > max {
        let end = first
            .char_indices()
            .nth(max)
            .map_or(first.len(), |(i, _)| i);
        return format!("{}…", first.get(..end).unwrap_or(first));
    }
    if more {
        format!("{first} …")
    } else {
        first.to_owned()
    }
}
