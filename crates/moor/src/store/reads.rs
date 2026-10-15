//! Read-only lookups by task name and live-box scans, built on the shared task row mapping.

use moor_core::domain::boxes::BoxState;
use moor_core::domain::task::TaskState;
use moor_core::{ProviderId, TaskId, TaskName};
use rusqlite::Connection;

use super::{
    event_from_row, task_from_row, BoxRow, EventRow, Store, StoreError, TaskRow, TASK_SELECT,
};

/// Run a task query and map every row.
fn query_tasks(
    conn: &Connection,
    tail: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<TaskRow>, StoreError> {
    let mut stmt = conn.prepare(&format!("{TASK_SELECT} {tail}"))?;
    let rows = stmt.query_and_then(params, task_from_row)?;
    rows.collect()
}

/// Box states that count toward capacity, as SQL parameters.
const LIVE_STATES: [&str; 3] = [
    BoxState::Starting.as_str(),
    BoxState::Ready.as_str(),
    BoxState::Asleep.as_str(),
];

/// Tasks holding a live Box of `provider` as seen on `conn`, oldest id first.
pub(super) fn live_holders_in(
    conn: &Connection,
    provider: &ProviderId,
) -> Result<Vec<TaskRow>, StoreError> {
    let tail = "WHERE b.provider = ?1 AND b.state IN (?2, ?3, ?4) ORDER BY t.id";
    let [a, b, c] = LIVE_STATES;
    query_tasks(conn, tail, (provider.as_str(), a, b, c))
}

/// An event with the name of the Task that owns it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedEvent {
    /// The stored event.
    pub(crate) row: EventRow,
    /// Name of the owning Task.
    pub(crate) task_name: String,
}

impl Store {
    /// Events with id greater than `after`, ascending by id, at most `limit`.
    pub(crate) fn events_after(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<Vec<NamedEvent>, StoreError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT e.id, e.task, e.kind, e.payload, e.ts, t.name FROM events e \
             JOIN tasks t ON t.id = e.task WHERE e.id > ?1 ORDER BY e.id LIMIT ?2",
        )?;
        let rows = stmt.query_and_then((after, limit), |r| {
            Ok(NamedEvent {
                row: event_from_row(r)?,
                task_name: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// The live (neither closed nor rejected) task with this name.
    pub(crate) fn open_task_by_name(&self, name: &TaskName) -> Result<Option<TaskRow>, StoreError> {
        let tail = "WHERE t.name = ?1 AND t.state NOT IN ('closed', 'rejected') ORDER BY t.id DESC LIMIT 1";
        let rows = query_tasks(&self.reader()?, tail, [name.as_str()])?;
        Ok(rows.into_iter().next())
    }

    /// The newest task with this name, open or closed.
    pub(crate) fn latest_task_by_name(
        &self,
        name: &TaskName,
    ) -> Result<Option<TaskRow>, StoreError> {
        let tail = "WHERE t.name = ?1 ORDER BY t.id DESC LIMIT 1";
        let rows = query_tasks(&self.reader()?, tail, [name.as_str()])?;
        Ok(rows.into_iter().next())
    }

    /// Boxes whose state counts toward capacity (starting, ready, asleep), with their task.
    pub(crate) fn box_rows_live(&self) -> Result<Vec<(TaskId, BoxRow)>, StoreError> {
        let tail = "WHERE b.state IN (?1, ?2, ?3) ORDER BY t.id";
        let rows = query_tasks(&self.reader()?, tail, LIVE_STATES)?;
        Ok(rows
            .into_iter()
            .filter_map(|t| t.boxed.map(|b| (t.id, b)))
            .collect())
    }

    /// Tasks queued for a box, oldest id first.
    pub(crate) fn waiting_tasks(&self) -> Result<Vec<TaskRow>, StoreError> {
        let tail = "WHERE t.state = ?1 ORDER BY t.id";
        query_tasks(&self.reader()?, tail, [TaskState::WaitingForBox.as_str()])
    }
}

#[cfg(test)]
mod tests;
