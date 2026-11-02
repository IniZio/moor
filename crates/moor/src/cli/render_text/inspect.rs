//! Text form of the `inspect` value: a header, the stuck reason, what the agent said, the pane.

use std::fmt::Write as _;

use serde_json::Value;

/// Renders an `inspect` value, or `None` when it lacks the expected shape.
pub(super) fn inspect_text(value: &Value) -> Option<String> {
    let mut out = header(value)?;
    if let Some(stuck) = present(value, "stuck") {
        let _ = writeln!(
            out,
            "stuck: {} since {}: {}",
            str_of(stuck, "kind")?,
            str_of(stuck, "since")?,
            str_of(stuck, "detail")?
        );
    }
    said(&mut out, value.get("said")?.as_array()?)?;
    if let Some(pane) = present(value, "pane") {
        tail(&mut out, pane)?;
    }
    Some(out)
}

/// A field that is present and not null.
fn present<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|f| !f.is_null())
}

/// A string field.
fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}

/// `<box> · <lifecycle>` plus the agent's state and source when it has one.
fn header(value: &Value) -> Option<String> {
    let mut out = format!(
        "{} · {}",
        str_of(value, "box")?,
        str_of(value, "lifecycle")?
    );
    if let Some(agent) = present(value, "agent") {
        let _ = write!(
            out,
            " · agent {} ({})",
            str_of(agent, "state")?,
            str_of(agent, "source")?
        );
    }
    out.push('\n');
    Some(out)
}

/// The `said:` block, each message prefixed by its time; nothing when there are none.
fn said(out: &mut String, entries: &[Value]) -> Option<()> {
    if entries.is_empty() {
        return Some(());
    }
    out.push_str("said:\n");
    for entry in entries {
        let at = str_of(entry, "at")?;
        let mut lines = str_of(entry, "text")?.lines();
        let _ = writeln!(out, "  {at}  {}", lines.next().unwrap_or(""));
        for line in lines {
            let _ = writeln!(out, "  {:width$}  {line}", "", width = at.len());
        }
    }
    Some(())
}

/// The pane heading and its tail.
fn tail(out: &mut String, pane: &Value) -> Option<()> {
    let text = str_of(pane, "tail")?;
    let _ = writeln!(
        out,
        "pane ({}, last {} lines):",
        str_of(pane, "status")?,
        text.lines().count()
    );
    out.push_str(text);
    if !text.is_empty() && !text.ends_with('\n') {
        out.push('\n');
    }
    Some(())
}
