//! Read-only lookups by task name and live-box scans, built on the shared task row mapping.

use moor_core::TaskId;
use moor_core::TaskName;
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
        let tail = "WHERE b.state IN ('starting', 'ready', 'asleep') ORDER BY t.id";
        let rows = query_tasks(&self.reader()?, tail, [])?;
        Ok(rows
            .into_iter()
            .filter_map(|t| t.boxed.map(|b| (t.id, b)))
            .collect())
    }
}

#[cfg(test)]
mod tests;
