//! Store rows to the core views (design §10.2).

use moor_core::domain::boxes::BoxState;
use moor_core::seam::clock::Ts;
use moor_core::view::{BoxView, EventView, TaskView};

use crate::store::{BoxRow, EventRow, TaskRow};

#[cfg(test)]
mod tests;

/// Format a timestamp as RFC3339 UTC with second precision.
pub(crate) fn rfc3339(ts: Ts) -> String {
    let secs = ts.as_millis().div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Civil date from days since 1970-01-01 (proleptic Gregorian).
const fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
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
    }
}

/// Map a Task row to the shared Task view; `agent` and `run` stay empty.
pub(crate) fn task_view(row: &TaskRow) -> TaskView {
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
        run: None,
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
