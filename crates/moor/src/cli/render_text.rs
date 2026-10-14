//! Human-readable rendering of a response envelope.

use std::fmt::Write as _;

use moor_core::view::env::{EnvCredential, EnvRow, EnvView};
use moor_core::{ExitClass, Response};
use serde_json::Value;

mod show;

/// Renders `resp` as `(stdout, stderr)` text; warnings are dropped when `quiet`.
pub(crate) fn render(resp: &Response, quiet: bool) -> (String, String) {
    let mut out = String::new();
    let mut err = String::new();
    if resp.class == ExitClass::Ok {
        // stdout of `run` is the program's own output; moor adds none.
        if resp.cmd != "run" {
            render_value(resp, &mut out);
        }
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
    if resp.cmd == "sleep" || resp.cmd == "wake" {
        if let Some(line) = sleep_line(&resp.cmd, value) {
            out.push_str(&line);
            return;
        }
    }
    if resp.cmd == "steer" {
        if let Some(line) = steer_line(value) {
            out.push_str(&line);
            return;
        }
    }
    if resp.cmd == "cp" {
        if let Some(text) = value.get("confirmation").and_then(Value::as_str) {
            out.push_str(text);
            out.push('\n');
            return;
        }
    }
    if let Some(text) = structured_text(&resp.cmd, value) {
        out.push_str(&text);
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(value) {
        out.push_str(&text);
        out.push('\n');
    }
}

/// Text form of the commands whose value is a structured view, or `None` to fall back.
fn structured_text(cmd: &str, value: &Value) -> Option<String> {
    match cmd {
        "env" => serde_json::from_value::<EnvView>(value.clone())
            .ok()
            .map(|view| env_text(&view)),
        "show" => show::show_text(value),
        _ => None,
    }
}

/// Formats the version line, or `None` when a field is missing or mistyped.
fn version_line(value: &Value) -> Option<String> {
    let version = value.get("version")?.as_str()?;
    let commit = value.get("commit")?.as_str()?;
    let schema = value.get("schema")?.as_u64()?;
    Some(format!("moor {version} {commit} schema={schema}\n"))
}

/// Formats the `sleep`/`wake` line, or `None` when the task name is missing or mistyped.
fn sleep_line(cmd: &str, value: &Value) -> Option<String> {
    let task = value.get("task")?.as_str()?;
    if cmd == "wake" {
        let woke = value.get("woke")?.as_bool()?;
        return Some(if woke {
            format!("{task} woke\n")
        } else {
            format!("{task} was not asleep\n")
        });
    }
    let state = value.pointer("/box/state").and_then(Value::as_str);
    Some(match state {
        Some(s) if s != "asleep" => format!("{task}: box is {s}\n"),
        _ => format!("{task} is asleep\n"),
    })
}

/// Formats the `steer` line, or `None` when the task name is missing or mistyped.
fn steer_line(value: &Value) -> Option<String> {
    let task = value.get("task")?.as_str()?;
    Some(format!("steered {task}\n"))
}

/// Text form of the `env` view: agent, github, token lines, then one line per item.
fn env_text(view: &EnvView) -> String {
    let mut out = String::from("agent:");
    let _ = writeln!(
        out,
        " model={}",
        view.agent.model.as_deref().unwrap_or("default")
    );
    let gh = &view.github;
    let _ = writeln!(
        out,
        "github: identity {}, app {}, {}",
        gh.identity,
        gh.app_id.as_deref().unwrap_or("none"),
        gh.login
    );
    let res = &gh.resolved;
    match (&res.mode, &res.token_source) {
        (Some(mode), Some(src)) => {
            let _ = writeln!(
                out,
                "github identity: {mode}, app installed {}, token from {src}",
                res.app_installed
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "github identity: unknown: {}",
                res.error.as_deref().unwrap_or("unresolved")
            );
        }
    }
    let mint = &gh.mint;
    match mint.status.as_str() {
        "failed" => {
            let _ = writeln!(
                out,
                "github token: failed: {}",
                mint.error.as_deref().unwrap_or("")
            );
        }
        "skipped" => {
            let _ = writeln!(
                out,
                "github token: skipped: {}",
                mint.reason.as_deref().unwrap_or("")
            );
        }
        other => {
            let _ = writeln!(out, "github token: {other}");
        }
    }
    for row in &view.rows {
        let _ = writeln!(out, "{}", row_line(row, view.check_of(row)));
    }
    out
}

/// One row line: mark, kind, name, layer, then optional note, delivery, reason.
fn row_line(row: &EnvRow, check: Option<&EnvCredential>) -> String {
    let mark = match check.map(|c| c.ok) {
        Some(true) => "\u{2713}",
        Some(false) => "\u{2717}",
        None => "-",
    };
    let mut line = format!(
        "{mark} {} {}  [{}]",
        row.kind.as_str(),
        row.name,
        row.source.as_str()
    );
    if row.note != row.name {
        let _ = write!(line, "  {}", row.note);
    }
    if let Some(d) = &row.delivery {
        let _ = write!(line, "  delivery: {d}");
    }
    if let Some(reason) = check.filter(|c| !c.ok).and_then(|c| c.reason.as_deref()) {
        let _ = write!(line, "  ({reason})");
    }
    line
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
