//! The workflow run a Box came from, so the page can offer to run it again.

use moor_core::progress::Progress;
use serde_json::{json, Value};

use super::super::{workflows, Session};
use super::Row;
use crate::Gate;

/// The workflow and repo of the run that made `row`'s Box, when the Box has stopped working, the
/// run is the latest of its repo and it went wrong; null otherwise. A later run has taken the
/// place of an older one, so there is nothing to run again.
pub(super) fn failed_run_of(gate: &Gate, session: &Session, row: Option<&Row>) -> Value {
    let Some(row) = row else {
        return Value::Null;
    };
    let state = row.progress.as_ref().map(|p| p.state);
    let over = matches!(
        state,
        Some(Progress::Failed | Progress::Closed | Progress::Done | Progress::Asleep)
    );
    if !over {
        return Value::Null;
    }
    let all = workflows::listed(gate, session).unwrap_or_default();
    all.iter()
        .find_map(|w| {
            let name = Some(row.name.as_str());
            let run = w
                .last_runs
                .iter()
                .find(|r| r.box_name.as_deref() == name || r.planned_box.as_deref() == name)?;
            workflows::went_wrong(run).then(|| json!({"name": w.name, "repo": run.repo}))
        })
        .unwrap_or(Value::Null)
}
