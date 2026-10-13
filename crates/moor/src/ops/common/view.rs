//! Store rows to the core views.

use moor_core::activity::AgentState;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::clock::Ts;
use moor_core::view::{AgentView, BoxSleepView, BoxView, EventView, RunView, TaskView};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::ops::common::activity::Reading;
use crate::store::{BoxRow, EventRow, RunRow, TaskRow};

#[cfg(test)]
mod tests;

/// Format a timestamp as RFC3339 UTC with second precision.
///
/// A timestamp outside years 0..=9999 (a corrupt row) renders as its raw
/// millisecond count in decimal.
pub(crate) fn rfc3339(ts: Ts) -> String {
    let ms = ts.as_millis();
    OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
}

/// Map a reading to the agent view; none when the Task has no agent.
pub(crate) fn agent_view(reading: &Reading) -> Option<AgentView> {
    let activity = reading.activity.as_ref()?;
    let (exit_code, waiting_for) = match &activity.state {
        AgentState::Exited { code } => (Some(*code), None),
        AgentState::Blocked { waiting_for } => (None, Some(waiting_for.clone())),
        _ => (None, None),
    };
    Some(AgentView {
        pane: reading.pane.as_str().to_owned(),
        state: activity.state.as_str().to_owned(),
        source: activity.source.as_str().to_owned(),
        exit_code,
        waiting_for,
    })
}

/// The handle to show: live, or the attempted one once the Box failed.
fn box_handle(b: &BoxRow) -> Option<String> {
    let h = match b.state {
        BoxState::Gone | BoxState::Deleted => None,
        BoxState::Failed | BoxState::ReleaseFailed => {
            b.handle.as_ref().or(b.attempted_handle.as_ref())
        }
        _ => b.handle.as_ref(),
    };
    h.map(|h| h.as_str().to_owned())
}

/// Map a Box row to its view.
fn box_view(b: &BoxRow) -> BoxView {
    BoxView {
        state: b.state,
        provider: b.provider.as_str().to_owned(),
        handle: box_handle(b),
        egress: b
            .egress_enforced
            .map(|e| (if e { "enforced" } else { "not_enforced" }).to_owned()),
        asleep_since: (b.state == BoxState::Asleep).then(|| rfc3339(b.updated_at)),
        sleep: BoxSleepView {
            after: None,
            reason: b.sleep_reason.clone(),
        },
    }
}

/// Map an in-flight run to its view.
fn run_view(r: &RunRow) -> RunView {
    RunView {
        id: r.id.get(),
        argv: r.argv.clone(),
        started_at: rfc3339(r.started_at),
    }
}

/// Map a Task row to the shared Task view; `agent` and `run` stay empty.
pub(crate) fn task_view(row: &TaskRow) -> TaskView {
    task_view_with_run(row, None)
}

/// Map a Task row to the shared Task view, showing `run` when one is in flight.
///
/// `agent` stays empty.
pub(crate) fn task_view_with_run(row: &TaskRow, run: Option<&RunRow>) -> TaskView {
    TaskView {
        id: row.id.get(),
        name: row.name.as_str().to_owned(),
        repo: row.repo.clone(),
        issue: row.issue.clone(),
        goal: row.goal.clone(),
        state: row.state,
        state_reason: row.state_reason.clone(),
        base: row.base.clone(),
        branch: row.branch.clone(),
        worktree: row.worktree.clone(),
        workspace: row.workspace_id.clone(),
        r#box: row.boxed.as_ref().map(box_view),
        agent: None,
        run: run.map(run_view),
        updated_at: rfc3339(row.updated_at),
    }
}

/// Map an event row to its view.
pub(crate) fn event_view(row: &EventRow) -> EventView {
    EventView {
        id: row.id,
        kind: row.kind.clone(),
        payload: row.payload.clone(),
        ts: rfc3339(row.ts),
    }
}
