//! The line each event worth telling becomes.

use serde_json::Value;

/// Events worth a line; the rest (busy, ready, asleep) are noise to a reader.
const WORTH: &[&str] = &[
    "idled",
    "needs_input",
    "agent_stalled",
    "agent_resumed",
    "node_disk_low",
    "node_disk_recovered",
    "setup_failed",
    "session_failed",
    "launch_timed_out",
    "box_failed",
    "relaunch_capped",
    "death_capped",
    "disk_lost",
    "session_restored",
    "session_lost",
    "closed",
    "run_waiting",
    "run_failed",
];

/// Longest the line's agent text runs.
pub(super) const SAID: usize = 200;

/// The first line of agent or owner text, trimmed and cut to [`SAID`] characters; none when it
/// is empty.
pub(super) fn said(text: &str) -> Option<String> {
    let first = text.lines().next()?.trim();
    (!first.is_empty()).then(|| first.chars().take(SAID).collect())
}

/// The newest event worth a line. The turn that stops on a question also ends idle; that idle
/// is the question, not news.
pub(super) fn newest_worth(events: &[Value]) -> Option<&Value> {
    fn kind(e: &Value) -> Option<&str> {
        e.get("kind").and_then(Value::as_str)
    }
    let mut newest = events
        .iter()
        .rev()
        .skip_while(|e| !kind(e).is_some_and(|k| WORTH.contains(&k)));
    let event = newest.next()?;
    match (kind(event), newest.next()) {
        (Some("idled"), Some(before)) if kind(before) == Some("needs_input") => Some(before),
        _ => Some(event),
    }
}

/// The line for a question the agent waits on, with its options; none for other kinds. A
/// question that could not be read gives a bare line, never the agent's earlier reply.
pub(super) fn question_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    if kind != "needs_input" {
        return None;
    }
    let question = payload
        .and_then(|p| p.get("question"))
        .and_then(Value::as_str)
        .filter(|q| !q.is_empty());
    let options: Vec<&str> = payload
        .and_then(|p| p.get("options"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    Some(match (question, options.is_empty()) {
        (None, _) => format!("moor: box {name} needs_input"),
        (Some(q), true) => format!("moor: box {name} needs_input: {q}"),
        (Some(q), false) => format!("moor: box {name} needs_input: {q} ({})", options.join(", ")),
    })
}

/// The line for a stall or its end, from the event's payload; none for other kinds.
pub(super) fn quiet_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    let mins = payload?.get("quiet_secs")?.as_u64()? / 60;
    match kind {
        "agent_stalled" => {
            let doing = payload?.get("doing").and_then(Value::as_str);
            Some(doing.filter(|d| !d.is_empty()).map_or_else(
                || format!("moor: box {name} stalled: no progress for {mins} min"),
                |d| format!("moor: box {name} stalled: no progress for {mins} min while {d}"),
            ))
        }
        "agent_resumed" => Some(format!("moor: box {name} resumed after {mins} min")),
        _ => None,
    }
}

/// The line for a Box whose machine died, so moor did not resume it; none for other kinds.
pub(super) fn death_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    if kind != "death_capped" {
        return None;
    }
    let payload = payload?;
    let why = payload.get("reason")?.as_str()?;
    Some(format!(
        "moor: box {name} death_capped: its machine stopped and moor did not resume it, wake it \
         or message it to bring it back: {why}"
    ))
}

/// The line for a lost machine, what came back of the conversation, or a node's disk running
/// low or coming back; none for other kinds.
pub(super) fn disk_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    let lost = match kind {
        "disk_lost" => Some("its machine was replaced, so anything not pushed is gone"),
        "session_restored" => Some(
            "its conversation up to its last saved turn is back; a prompt sent after that was \
             not sent again",
        ),
        "session_lost" => Some(
            "its conversation could not be brought back, so its agent started without it; a \
             prompt it was working on was not sent again",
        ),
        _ => None,
    };
    if let Some(what) = lost {
        return Some(format!("moor: box {name} {kind}: {what}"));
    }
    if !matches!(kind, "node_disk_low" | "node_disk_recovered") {
        return None;
    }
    let payload = payload?;
    let node = payload.get("node")?.as_str()?;
    let free = payload.get("free_mib")?.as_u64()? / 1024;
    let total = payload.get("total_mib")?.as_u64()? / 1024;
    Some(format!(
        "moor: box {name} {kind}: {node} has {free} GiB free of {total} GiB"
    ))
}

/// The line for a workflow run that waits for its owner or ended badly, with the run page; none
/// for other kinds.
pub(super) fn run_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    let text = |key| payload?.get(key)?.as_str();
    let said = |key| text(key).map(|t| said(t).unwrap_or_default());
    let (workflow, repo, link) = (text("workflow")?, text("repo")?, text("link")?);
    match kind {
        "run_waiting" => Some(format!(
            "moor: box {name} run_waiting: {workflow} ({repo}) asks: {} {link}",
            said("question")?
        )),
        "run_failed" => {
            let at = text("step").map_or_else(String::new, |s| format!(" at {s}"));
            Some(format!(
                "moor: box {name} run_failed: {workflow} ({repo}) {}{at}: {} {link}",
                text("outcome")?,
                said("reason")?
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
