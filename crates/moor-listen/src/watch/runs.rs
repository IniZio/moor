//! The workflows a listen follows: each one's place in its change stream, where each run stood
//! when last read, and the line each move of a run becomes.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{json, Value};

use super::line::said;
use crate::gate::Gate;

/// Every workflow's runs URI starts with this; the workflow's name follows.
pub(super) const RUNS: &str = "moor://workflow/";

/// Most changes one read asks for.
const PAGE: u64 = 200;

/// Where a run stood: its phase and the step it was at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    /// `queued`, `running`, `waiting` or `done`.
    phase: String,
    /// The step's id; none when it is at no step.
    step: Option<String>,
}

/// The workflows followed and what was read of them.
pub(super) struct Runs {
    /// Each workflow followed, with the `next` of its last read; none before the first.
    cursors: Vec<(String, Option<i64>)>,
    /// Where each run read so far stood, by run id.
    seen: HashMap<i64, Place>,
}

impl Runs {
    /// Follows the workflows of these names.
    pub(super) fn new(names: Vec<String>) -> Self {
        Self {
            cursors: names.into_iter().map(|n| (n, None)).collect(),
            seen: HashMap::new(),
        }
    }

    /// Whether any workflow is followed.
    pub(super) const fn any(&self) -> bool {
        !self.cursors.is_empty()
    }

    /// The URIs to listen on.
    pub(super) fn uris(&self) -> impl Iterator<Item = String> + '_ {
        self.cursors.iter().map(|(n, _)| format!("{RUNS}{n}"))
    }

    /// The names of the workflows followed.
    pub(super) fn names(&self) -> Vec<String> {
        self.cursors.iter().map(|(n, _)| n.clone()).collect()
    }

    /// Reads workflow `name`'s changes since its last read and returns a line for each move.
    /// The first read of a workflow is history, not news: it notes where each run stood and
    /// returns nothing. A read that fails returns what came before it, and the next read
    /// starts again from there.
    pub(super) fn news(&mut self, gate: &Gate, token: &str, name: &str) -> Vec<String> {
        let Some(at) = self.cursors.iter().position(|(n, _)| n == name) else {
            return Vec::new();
        };
        let first = self.cursors.get(at).and_then(|(_, c)| *c).is_none();
        let mut lines = Vec::new();
        loop {
            let after = self.cursors.get(at).and_then(|(_, c)| *c).unwrap_or(0);
            let Some((changes, next)) = read(gate, token, name, after) else {
                break;
            };
            let full = changes.len() as u64 >= PAGE;
            for change in &changes {
                let said = self.moved(name, change);
                if !first {
                    lines.extend(said);
                }
            }
            if let Some((_, cursor)) = self.cursors.get_mut(at) {
                *cursor = Some(next);
            }
            if !full {
                break;
            }
        }
        lines
    }

    /// Notes where the run in `change` stands and returns the lines for how it got there.
    fn moved(&mut self, workflow: &str, change: &Value) -> Vec<String> {
        let Some(id) = change.get("id").and_then(Value::as_i64) else {
            return Vec::new();
        };
        let text = |key: &str| change.get(key).and_then(Value::as_str);
        let place = Place {
            phase: text("phase").unwrap_or_default().to_owned(),
            step: text("step").map(str::to_owned),
        };
        let before = self.seen.insert(id, place.clone());
        let line = |event: &str, about: Option<&str>| line(workflow, id, event, about, change);
        let step = text("step_title").or_else(|| text("step"));
        let mut lines = Vec::new();
        if before.is_none() && place.phase != "done" {
            lines.push(line("started", text("trigger")));
        }
        let was = |phase: &str| before.as_ref().is_some_and(|b| b.phase == phase);
        match place.phase.as_str() {
            "running"
                if !was("running") || before.as_ref().is_some_and(|b| b.step != place.step) =>
            {
                lines.push(line("step", step));
            }
            "waiting" if !was("waiting") => lines.push(line("waiting", step)),
            "done" if !was("done") => {
                let outcome = text("outcome").unwrap_or("ended");
                let about = text("result").or_else(|| text("detail"));
                lines.push(line(&format!("ended:{outcome}"), about));
            }
            _ => {}
        }
        lines
    }
}

/// The line for run `id` of `workflow`: `event`, the first line of `about`, and the pull
/// request the run is for, as `OWNER/NAME#N@SHA7`.
fn line(workflow: &str, id: i64, event: &str, about: Option<&str>, change: &Value) -> String {
    let mut out = format!("moor: run {workflow} {id} {event}");
    if let Some(about) = about.and_then(said) {
        out.push_str(": ");
        out.push_str(&about);
    }
    if let Some(pull) = change.get("pull").and_then(Value::as_str) {
        let sha = change
            .get("head_sha")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let short: String = sha.chars().take(7).collect();
        let _ = write!(out, " ({pull}@{short})");
    }
    out
}

/// Workflow `name`'s changes after `after`, and the cursor to read from next; none when the
/// read failed.
fn read(gate: &Gate, token: &str, name: &str, after: i64) -> Option<(Vec<Value>, i64)> {
    let args = json!({"workflow": name, "after": after, "limit": PAGE});
    let params = json!({"name": "list_workflow_runs", "arguments": args});
    let result = gate.call(token, "tools/call", params).ok()?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let value = result.get("structuredContent")?;
    let changes = value.get("changes")?.as_array()?.clone();
    Some((changes, value.get("next")?.as_i64()?))
}
