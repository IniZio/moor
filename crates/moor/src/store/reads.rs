//! Read-only lookups by task name and live-box scans, built on the shared task row mapping.

use moor_core::domain::boxes::BoxState;
use moor_core::domain::task::TaskState;
use moor_core::{ProviderId, TaskId, TaskName};
use rusqlite::Connection;

use super::{task_from_row, BoxRow, Store, StoreError, TaskRow, TASK_SELECT};

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

impl Store {
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

    /// Every task, by id ascending.
    pub(crate) fn all_tasks(&self) -> Result<Vec<TaskRow>, StoreError> {
        query_tasks(&self.reader()?, "ORDER BY t.id", [])
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

    /// How many live boxes this provider holds.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by capacity logic from S5")
    )]
    pub(crate) fn live_boxes(&self, provider: &ProviderId) -> Result<u32, StoreError> {
        let sql = "SELECT COUNT(*) FROM boxes WHERE provider = ?1 AND state IN (?2, ?3, ?4)";
        let [a, b, c] = LIVE_STATES;
        Ok(self
            .reader()?
            .query_row(sql, (provider.as_str(), a, b, c), |r| r.get(0))?)
    }

    /// Tasks queued for a box, oldest id first.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by capacity logic from S5")
    )]
    pub(crate) fn waiting_tasks(&self) -> Result<Vec<TaskRow>, StoreError> {
        let tail = "WHERE t.state = ?1 ORDER BY t.id";
        query_tasks(&self.reader()?, tail, [TaskState::WaitingForBox.as_str()])
    }
}

#[cfg(test)]
mod tests;
