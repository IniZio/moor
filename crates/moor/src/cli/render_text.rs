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
        "start" => start_text(value),
        "close" => close_text(value),
        "wait" => wait_text(value),
        "end-session" => end_session_line(value),
        "open-session" => open_session_line(value),
        "open-host-session" | "update-host-session" | "end-host-session" => {
            host_session_line(value)
        }
        _ => None,
    }
}

/// `(name (#id), state)` for a task row, or `None` when name, id or state is missing.
fn task_head(value: &Value) -> Option<(String, String)> {
    let task = value.get("task")?;
    let name = task.get("name")?.as_str()?;
    let id = task.get("id")?.as_i64()?;
    let state = task.get("state")?.as_str()?;
    Some((format!("{name} (#{id})"), state.to_owned()))
}

/// Writes `started from the queue: a, b` when the `promoted` names are non-empty.
fn promoted_line(value: &Value, out: &mut String) {
    let names: Vec<&str> = value
        .get("promoted")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !names.is_empty() {
        let _ = writeln!(out, "started from the queue: {}", names.join(", "));
    }
}

/// Text form of `start`: a status line, how to open a Task that has a Box, then notes.
fn start_text(value: &Value) -> Option<String> {
    let (head, state) = task_head(value)?;
    let flag = |key| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let task = value.get("task")?;
    let box_state = task.pointer("/box/state").and_then(Value::as_str);
    let mut out = if flag("queued") {
        format!("queued {head}: waiting for a free box\n")
    } else {
        let verb = if flag("attached") {
            "attached"
        } else {
            "started"
        };
        let box_text = box_state.map_or_else(|| "no box".to_owned(), |s| format!("box {s}"));
        let setup = value.get("setup");
        let setup_flag = |key| setup.and_then(|s| s.get(key)).and_then(Value::as_bool);
        let failed = setup_flag("ran") == Some(true) && setup_flag("ok") == Some(false);
        let suffix = if failed { ", setup failed" } else { "" };
        format!("{verb} {head}: {state}, {box_text}{suffix}\n")
    };
    if matches!(box_state, Some("ready" | "starting" | "asleep")) {
        let name = task.get("name")?.as_str()?;
        let _ = writeln!(out, "open it with: moor open {name}");
    }
    setup_failure_lines(value, &mut out);
    promoted_line(value, &mut out);
    for w in value
        .get("warnings_inline")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let _ = writeln!(out, "warning: {w}");
    }
    Some(out)
}

/// Writes the failed step's log tail, the Box it is in and how to close it, for a start that
/// carried on past a failed step.
fn setup_failure_lines(value: &Value, out: &mut String) {
    let Some(step) = value.get("failed_step").and_then(Value::as_str) else {
        return;
    };
    let handle = value.get("box").and_then(Value::as_str);
    let _ = match handle {
        Some(h) => writeln!(out, "{step} failed in Box {h}:"),
        None => writeln!(out, "{step} failed:"),
    };
    for l in value
        .pointer("/setup/tail")
        .and_then(Value::as_str)
        .into_iter()
        .flat_map(str::lines)
    {
        let _ = writeln!(out, "  {l}");
    }
    if let Some(name) = value.pointer("/task/name").and_then(Value::as_str) {
        let _ = writeln!(out, "close it with: moor close {name}");
    }
}

/// Text form of `close`: a status line, the transcript path, then queue promotions.
fn close_text(value: &Value) -> Option<String> {
    let (head, _) = task_head(value)?;
    let deleted = value.get("box_deleted")?.as_bool()?;
    let outcome = if deleted { "box deleted" } else { "box kept" };
    let mut out = format!("closed {head}, {outcome}\n");
    if let Some(path) = value.get("transcript").and_then(Value::as_str) {
        let _ = writeln!(out, "transcript: {path}");
    }
    promoted_line(value, &mut out);
    Some(out)
}

/// Compact duration such as `250ms`, `42s`, `4m12s` or `1h02m03s`.
fn duration_text(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0 => format!("{ms}ms"),
        1..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m{:02}s", secs / 3600, secs % 3600 / 60, secs % 60),
    }
}

/// Text form of `wait`: one line naming the condition, whether it held, and the time spent.
fn wait_text(value: &Value) -> Option<String> {
    let (head, _) = task_head(value)?;
    let cond = value.get("for")?.as_str()?;
    let after = duration_text(value.get("waited_ms")?.as_u64()?);
    if value.get("timed_out")?.as_bool()? {
        return Some(format!(
            "{head}: timed out waiting for {cond} after {after}\n"
        ));
    }
    Some(format!("{head}: {cond} after {after}\n"))
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

/// Formats the `end-session` line, or `None` when a field is missing or mistyped.
fn end_session_line(value: &Value) -> Option<String> {
    let task = value.get("task")?.as_str()?;
    let ordinal = value.get("ordinal")?.as_i64()?;
    let id = value.get("session")?.as_i64()?;
    let kept = if value.get("box_kept")?.as_bool()? {
        "Box kept open"
    } else {
        "Box not kept"
    };
    Some(format!(
        "ended Session {ordinal} (id {id}) in {task}; {kept}\n"
    ))
}

/// Formats the `open-session` line, or `None` when a field is missing or mistyped.
fn open_session_line(value: &Value) -> Option<String> {
    let task = value.get("task")?.as_str()?;
    let ordinal = value.get("ordinal")?.as_i64()?;
    let id = value.get("session")?.as_i64()?;
    let pane = value
        .get("pane")
        .and_then(Value::as_str)
        .map(|p| format!("; pane {p}"))
        .unwrap_or_default();
    Some(format!(
        "opened Session {ordinal} (id {id}) in {task}{pane}\n"
    ))
}

/// Formats the `open-host-session` line, or `None` when a field is missing or mistyped.
fn host_session_line(value: &Value) -> Option<String> {
    let id = value.get("session")?.as_i64()?;
    let workdir = value.get("workdir")?.as_str()?;
    Some(format!("host Session {id} working in {workdir}\n"))
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
