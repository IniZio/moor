//! Releases lease rows no live `moor close` is going to: a Task closed with `--force`, rejected,
//! or whose owner is gone leaves Box resources recorded that nothing else would remove.
//!
//! A Task that is still open is never touched. Rows whose last attempt failed wait out their
//! backoff (`next_attempt_at`) before the next try.

use moor_core::TaskId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::release::release_box;
use crate::store::lease::LeaseRow;
use crate::store::{Store, TaskRow};

/// Releases the orphan leases of every Task once; one log line per Task acted on or left.
///
/// A failure on one Task is a log line and never stops the others.
pub(crate) fn run(cx: &Cx) -> Result<Vec<String>, CommandError> {
    let store = cx.store()?;
    let now = cx.clock().now();
    let mut out = Vec::new();
    for (task, rows) in by_task(store.all_leases()?) {
        if !rows.iter().any(|r| r.due(now)) {
            continue;
        }
        if let Some(line) = reap_task(cx, store, task, &rows) {
            out.push(line);
        }
    }
    Ok(out)
}

/// Lease rows grouped by Task, in first-seen order.
fn by_task(rows: Vec<LeaseRow>) -> Vec<(TaskId, Vec<LeaseRow>)> {
    let mut groups: Vec<(TaskId, Vec<LeaseRow>)> = Vec::new();
    for row in rows {
        match groups.iter_mut().find(|(t, _)| *t == row.task) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((row.task, vec![row])),
        }
    }
    groups
}

/// Reaps one Task's rows; `None` when the Task is live and nothing was done.
fn reap_task(cx: &Cx, store: &Store, task: TaskId, rows: &[LeaseRow]) -> Option<String> {
    let id = task.get();
    let row = match store.task(task) {
        Ok(Some(row)) => row,
        Ok(None) => return Some(missing(id, rows)),
        Err(e) => return Some(format!("reap task {id}: {e}")),
    };
    if row.state.is_live() && !owner_gone(cx, store, &row) {
        return None;
    }
    Some(reap(cx, store, &row, rows).unwrap_or_else(|e| format!("reap task {id}: {}", e.error)))
}

/// Whether the owner of the open Task `row` is gone, with no live Proxy to speak for it.
///
/// No Task owner liveness exists yet, so this is `false` and an open Task is never reaped.
/// Reaping an open Task needs a real liveness check here first.
const fn owner_gone(_cx: &Cx, _store: &Store, _row: &TaskRow) -> bool {
    false
}

/// The log line for lease rows whose Task row is gone.
///
/// Releasing needs the Task's Driver handle, which only the Task row holds, and the foreign key
/// keeps rows from outliving it. The rows are left for a person to clear.
fn missing(id: i64, rows: &[LeaseRow]) -> String {
    format!(
        "reap task {id}: no task row, left {} lease row(s): {}",
        rows.len(),
        keys(rows)
    )
}

/// Releases the Box of `row` under its Box lock and says how it went.
fn reap(cx: &Cx, store: &Store, row: &TaskRow, rows: &[LeaseRow]) -> Result<String, CommandError> {
    let id = row.id.get();
    let Some(handle) = row
        .boxed
        .as_ref()
        .and_then(|b| b.handle.as_ref().or(b.attempted_handle.as_ref()))
    else {
        return Ok(format!(
            "reap task {id}: no Box handle recorded, left {} lease row(s): {}",
            rows.len(),
            keys(rows)
        ));
    };
    let lock = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, "reap", boxlock::WAIT);
    let _lock = match lock {
        Ok(lock) => lock,
        Err(e) => return Ok(format!("reap task {id}: skipped, {}", e.error)),
    };
    let report = release_box(cx, store, row, handle, false)?;
    Ok(match (&report.driver, report.released) {
        (Ok(_), true) => format!("reap task {id}: released {} lease row(s)", rows.len()),
        (Err(e), _) => format!("reap task {id}: Box release failed: {e}; will retry"),
        (Ok(_), false) => format!(
            "reap task {id}: left {}; will retry",
            if report.left.is_empty() {
                "lease rows".to_owned()
            } else {
                report.left.join("; ")
            }
        ),
    })
}

/// The kind and key of each row, for a log line.
fn keys(rows: &[LeaseRow]) -> String {
    rows.iter()
        .map(|r| format!("{}:{}", r.kind.as_str(), r.key))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
