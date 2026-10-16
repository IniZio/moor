//! Text form of the `show` value: a task table, or one task's detail block.

use std::fmt::Write as _;

use serde_json::Value;

#[cfg(test)]
mod tests;

/// Renders a `show` value, or `None` when it lacks the expected shape.
pub(super) fn show_text(value: &Value) -> Option<String> {
    let mut out = match value.get("task") {
        Some(task) if !task.is_null() => detail(task, value)?,
        _ => list(value.get("tasks")?.as_array()?)?,
    };
    if let Some(pane) = value.get("pane").filter(|p| !p.is_null()) {
        let tail = pane.get("tail")?.as_str()?;
        out.push_str("--- pane ---\n");
        out.push_str(tail);
        if !tail.ends_with('\n') {
            out.push('\n');
        }
    }
    Some(out)
}

/// A string field, or `-` when absent or null.
fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("-")
}

/// One aligned line per task.
fn list(tasks: &[Value]) -> Option<String> {
    if tasks.is_empty() {
        return Some("no tasks\n".to_owned());
    }
    let rows: Vec<[&str; 5]> = tasks
        .iter()
        .map(|t| {
            Some([
                t.get("name")?.as_str()?,
                field(t, "state"),
                t.pointer("/agent/state")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                t.pointer("/box/state")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                t.pointer("/box/workdir")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
            ])
        })
        .collect::<Option<_>>()?;
    let mut widths = [0usize; 4];
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.len());
        }
    }
    let mut out = String::new();
    for r in &rows {
        let _ = writeln!(
            out,
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
    }
    Some(out)
}

/// The agent lines of the detail block: state, then any question with its numbered options.
fn agent_block(out: &mut String, a: &Value) {
    let _ = writeln!(
        out,
        "agent:    {} pane={}",
        field(a, "state"),
        field(a, "pane")
    );
    if let Some(q) = a.get("question").and_then(Value::as_str) {
        let _ = writeln!(out, "  question: {q}");
    }
    let options = a.get("options").and_then(Value::as_array);
    for (i, o) in options
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
    {
        let _ = writeln!(out, "  {}. {o}", i + 1);
    }
}

/// The proof lines of the detail block: `name  size  sha256` per file.
fn proof_block(out: &mut String, files: &[Value]) {
    if files.is_empty() {
        out.push_str("proof:    none\n");
        return;
    }
    out.push_str("proof:\n");
    for f in files {
        let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
        let _ = writeln!(
            out,
            "  {}  {size}  {}",
            field(f, "name"),
            field(f, "sha256")
        );
    }
}

/// The detail block for one task.
fn detail(task: &Value, value: &Value) -> Option<String> {
    let mut out = String::new();
    let _ = writeln!(out, "name:     {}", task.get("name")?.as_str()?);
    let _ = writeln!(out, "state:    {}", field(task, "state"));
    let _ = writeln!(out, "repo:     {}", field(task, "repo"));
    let _ = writeln!(out, "worktree: {}", field(task, "worktree"));
    let _ = writeln!(out, "branch:   {}", field(task, "branch"));
    match task.get("box").filter(|b| !b.is_null()) {
        Some(b) => {
            let _ = writeln!(
                out,
                "box:      {} {} workdir={} egress={}",
                field(b, "provider"),
                field(b, "state"),
                field(b, "workdir"),
                field(b, "egress"),
            );
        }
        None => out.push_str("box:      none\n"),
    }
    if let Some(a) = task.get("agent").filter(|a| !a.is_null()) {
        agent_block(&mut out, a);
    }
    for key in ["setup", "dotfiles"] {
        let Some(s) = value
            .get(key)
            .or_else(|| task.get(key))
            .filter(|s| !s.is_null())
        else {
            continue;
        };
        let ran = s.get("ran").and_then(Value::as_bool).unwrap_or(false);
        let ok = s.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let status = match (ran, ok) {
            (false, _) => "not run",
            (true, true) => "ok",
            (true, false) => "failed",
        };
        let _ = writeln!(out, "{key}: {status}");
    }
    if let Some(files) = value.get("proofs").and_then(Value::as_array) {
        proof_block(&mut out, files);
    }
    if let Some(events) = value.get("events").and_then(Value::as_array) {
        out.push_str("events:\n");
        for e in events {
            let _ = writeln!(out, "  {}  {}", field(e, "ts"), field(e, "kind"));
        }
    }
    Some(out)
}
