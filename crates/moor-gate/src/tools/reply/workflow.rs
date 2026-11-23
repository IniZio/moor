//! What the workflow list tools send: one compact row each, so a listing stays cheap. The prompt,
//! config, gates, outcomes and run history stay with `get_workflow` and `get_workflow_run`.

use serde_json::{json, Value};

/// Longest title or refusal line a row keeps, in characters.
const LINE: usize = 120;

/// Shrinks `workflow-list`'s answer to one compact row per workflow.
pub(super) fn listed(value: &mut Value) {
    let Some(Value::Array(rows)) = value.get_mut("workflows") else {
        return;
    };
    for row in rows {
        *row = compact(row);
    }
}

/// Replaces `workflow-list`'s answer by the one workflow called `name`, in full.
pub(super) fn one(value: &mut Value, name: &str) -> Result<(), String> {
    let found = value
        .get_mut("workflows")
        .and_then(Value::as_array_mut)
        .and_then(|rows| {
            rows.iter_mut()
                .find(|r| r.get("name").is_some_and(|n| n == name))
        })
        .map(Value::take);
    *value =
        found.ok_or_else(|| format!("no workflow named {name}; list_workflows shows the names"))?;
    Ok(())
}

/// Shrinks `workflow-runs`'s answer to one compact row per change.
pub(super) fn runs(value: &mut Value) {
    let Some(Value::Array(rows)) = value.get_mut("changes") else {
        return;
    };
    for row in rows {
        *row = run_row(row);
    }
}

/// Replaces `workflow-run-list`'s answer by run `run` in full, with the workflow's name.
pub(super) fn whole_run(value: &mut Value, run: i64) -> Result<(), String> {
    let workflow = value.get("workflow").cloned().unwrap_or(Value::Null);
    let found = value
        .get_mut("runs")
        .and_then(Value::as_array_mut)
        .and_then(|rows| {
            rows.iter_mut()
                .find(|r| r.get("id").is_some_and(|i| i == run))
        })
        .map(Value::take);
    let mut found = found.ok_or_else(|| {
        format!("no run {run} of that workflow; list_workflow_runs shows run ids")
    })?;
    if let Some(fields) = found.as_object_mut() {
        fields.insert("workflow".to_owned(), workflow);
    }
    *value = found;
    Ok(())
}

/// The row `list_workflows` sends for the full workflow `row`: what a person scans for.
fn compact(row: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let mut keep = |name: &str| {
        if let Some(v) = row.get(name).filter(|v| !v.is_null()) {
            out.insert(name.to_owned(), v.clone());
        }
    };
    for name in ["name", "enabled", "cron", "timezone", "repos", "next_due"] {
        keep(name);
    }
    if let Some(last) = row.get("last_run").filter(|l| l.is_object()) {
        out.insert("last_run".to_owned(), json!(last_line(last)));
    }
    let gates: Vec<&str> = row
        .get("gates")
        .and_then(Value::as_array)
        .map(|g| g.iter().filter_map(|g| g.get("name")?.as_str()).collect())
        .unwrap_or_default();
    if !gates.is_empty() {
        out.insert("gates".to_owned(), json!(gates));
    }
    if let Some(title) = row.get("prompt").and_then(Value::as_str).and_then(title) {
        out.insert("title".to_owned(), json!(title));
    }
    Value::Object(out)
}

/// The heading a prompt opens with, when it opens with one.
fn title(prompt: &str) -> Option<String> {
    let first = prompt.lines().find(|l| !l.trim().is_empty())?.trim();
    let heading = first.strip_prefix('#')?.trim_start_matches('#').trim();
    (!heading.is_empty()).then(|| heading.chars().take(LINE).collect())
}

/// When a run was due, in UTC to the minute.
fn when(due_at: Option<&Value>) -> Option<String> {
    let at = jiff::Timestamp::from_millisecond(due_at?.as_i64()?).ok()?;
    Some(at.strftime("%Y-%m-%d %H:%M UTC").to_string())
}

/// The one line a workflow row says about its latest run: when, outcome and Box.
fn last_line(run: &Value) -> String {
    let outcome = run
        .get("outcome")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut parts = vec![
        when(run.get("due_at")).unwrap_or_default(),
        outcome.to_owned(),
    ];
    if let Some(verdict) = run
        .get("verdict")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
    {
        parts.push(format!("verdict {verdict}"));
    }
    if let Some(name) = run
        .get("box")
        .or_else(|| run.get("planned_box"))
        .and_then(Value::as_str)
    {
        parts.push(format!("Box {name}"));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// The row `list_workflow_runs` sends for the change `change`; the whole run is one
/// `get_workflow_run` away.
fn run_row(change: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for name in [
        "id",
        "workflow",
        "due_at",
        "outcome",
        "phase",
        "verdict",
        "trigger",
        "pull",
        "head_sha",
        "step",
        "step_title",
        "box",
    ] {
        if let Some(v) = change.get(name).filter(|v| !v.is_null()) {
            out.insert(name.to_owned(), v.clone());
        }
    }
    let why = change
        .get("detail")
        .and_then(Value::as_str)
        .and_then(|d| d.lines().find(|l| !l.trim().is_empty()));
    if let Some(why) = why {
        out.insert(
            "detail".to_owned(),
            json!(why.trim().chars().take(LINE).collect::<String>()),
        );
    }
    Value::Object(out)
}
