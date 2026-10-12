//! `gate`: answer whether a recorded run of the gate passed on a commit.

use moor_core::decide::gate::{decide, GateQuery, GateRun, GateVerdict};
use moor_core::{ErrorKind, GateReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::RunRow;

/// The verdict as the response value.
#[derive(Debug, serde::Serialize)]
pub(crate) struct GateView {
    /// Resolved full sha, lowercase.
    sha: String,
    /// Gate name.
    name: String,
    /// Command of the chosen run.
    command: Option<String>,
    /// Exit of the chosen run.
    exit: Option<i32>,
    /// Whether the gate passed.
    ok: bool,
    /// When the chosen run was recorded.
    recorded_at: Option<String>,
}

impl From<GateVerdict> for GateView {
    fn from(v: GateVerdict) -> Self {
        Self {
            sha: v.sha,
            name: v.name,
            command: v.command,
            exit: v.exit,
            ok: v.ok,
            recorded_at: v.recorded_at.map(rfc3339),
        }
    }
}

/// Run `gate`; needs only the store, so it works from any directory.
pub(crate) fn run(cx: &Cx, req: &GateReq) -> Result<GateView, CommandError> {
    let rows = cx.store()?.recorded_runs(&req.sha)?;
    let runs: Vec<GateRun> = rows.iter().filter_map(gate_run).collect();
    let query = GateQuery {
        sha: &req.sha,
        name: &req.name,
        command: req.command.as_deref(),
    };
    let verdict = decide(&query, &runs).map_err(|e| CommandError::new(e.kind(), e.to_string()))?;
    let view = GateView::from(verdict);
    if view.ok {
        return Ok(view);
    }
    let message = format!("gate `{}` has no passing run on {}", view.name, view.sha);
    let value =
        serde_json::to_value(&view).map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    Err(CommandError::new(ErrorKind::Failed, message).with_value(value))
}

/// A recorded run as the decider reads it; `None` for a row without gate name or sha.
fn gate_run(row: &RunRow) -> Option<GateRun> {
    Some(GateRun {
        id: row.id,
        sha: row.sha.as_ref()?.as_str().to_owned(),
        name: row.record.clone()?,
        command: row.argv.join(" "),
        exit: row.exit,
        interrupted: row.interrupted,
        recorded_at: row.finished_at.unwrap_or(row.started_at),
    })
}

#[cfg(test)]
mod tests;
