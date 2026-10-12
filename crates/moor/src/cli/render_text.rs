//! Human-readable rendering of a response envelope.

use std::fmt::Write as _;

use moor_core::{ExitClass, Response};
use serde_json::Value;

/// Renders `resp` as `(stdout, stderr)` text; warnings are dropped when `quiet`.
pub(crate) fn render(resp: &Response, quiet: bool) -> (String, String) {
    let mut out = String::new();
    let mut err = String::new();
    if resp.class == ExitClass::Ok {
        render_value(resp, &mut out);
    } else {
        render_error(resp, &mut err);
    }
    if !quiet {
        for w in &resp.warnings {
            let _ = writeln!(err, "moor: warning: {}", w.message);
        }
    }
    (out, err)
}

/// Writes the success value to `out`.
fn render_value(resp: &Response, out: &mut String) {
    let Some(value) = &resp.value else {
        return;
    };
    if resp.cmd == "version" {
        if let Some(line) = version_line(value) {
            out.push_str(&line);
            return;
        }
    }
    if let Ok(text) = serde_json::to_string_pretty(value) {
        out.push_str(&text);
        out.push('\n');
    }
}

/// Formats the version line, or `None` when a field is missing or mistyped.
fn version_line(value: &Value) -> Option<String> {
    let version = value.get("version")?.as_str()?;
    let commit = value.get("commit")?.as_str()?;
    let schema = value.get("schema")?.as_u64()?;
    Some(format!("moor {version} {commit} schema={schema}\n"))
}

/// Writes the error message and hints to `err`.
fn render_error(resp: &Response, err: &mut String) {
    let Some(e) = &resp.error else {
        let _ = writeln!(err, "moor: {} failed", resp.cmd);
        return;
    };
    let _ = writeln!(err, "moor: {}", e.message);
    for h in &e.hints {
        let _ = writeln!(err, "hint: {h}");
    }
}

#[cfg(test)]
mod tests;
