//! Report versions kept per task; contents live as files, rows hold the path.

use std::fs;
use std::io;
use std::path::Path;

use moor_core::seam::clock::Ts;
use moor_core::TaskId;
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};

use super::{Store, StoreError};

/// One stored version of a Task's Report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning task.
    pub(crate) task: TaskId,
    /// Report kind; versions are counted per kind.
    pub(crate) kind: String,
    /// Version number, starting at 1.
    pub(crate) version: i64,
    /// Full path of the file holding this version.
    pub(crate) path: String,
    /// Title, if one was recorded.
    pub(crate) title: Option<String>,
    /// When this version was stored.
    pub(crate) created_at: Ts,
}

impl Store {
    /// Stores `content` as the next version of `kind`, under `dir/<task id>/<kind>.v<version>.md`.
    /// Returns None and writes nothing when the latest stored version holds the same content.
    pub(crate) fn add_report(
        &self,
        task: TaskId,
        kind: &str,
        content: &str,
        dir: &Path,
    ) -> Result<Option<ReportRow>, StoreError> {
        if kind.is_empty() || kind.contains('/') || kind == "." || kind == ".." {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid report kind `{kind}`"),
            )));
        }
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest: Option<(i64, String)> = tx
            .query_row(
                "SELECT version, path FROM reports WHERE task = ? AND kind = ? \
                 ORDER BY version DESC LIMIT 1",
                params![task.get(), kind],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((_, path)) = &latest {
            if fs::read(path).is_ok_and(|old| old == content.as_bytes()) {
                return Ok(None);
            }
        }
        let version = latest.map_or(1, |(v, _)| v + 1);
        let task_dir = dir.join(task.get().to_string());
        fs::create_dir_all(&task_dir)?;
        let file = task_dir.join(format!("{kind}.v{version}.md"));
        let path = file.to_string_lossy().into_owned();
        fs::write(&file, content)?;
        let stored = tx
            .execute(
                "INSERT INTO reports (task, kind, version, path, title, created_at) \
                 VALUES (?, ?, ?, ?, NULL, ?)",
                params![task.get(), kind, version, path, now.as_millis()],
            )
            .map(|_| tx.last_insert_rowid())
            .and_then(|id| tx.commit().map(|()| id));
        let id = match stored {
            Ok(id) => id,
            Err(e) => {
                let _ = fs::remove_file(&file);
                return Err(e.into());
            }
        };
        drop(conn);
        Ok(Some(ReportRow {
            id,
            task,
            kind: kind.to_owned(),
            version,
            path,
            title: None,
            created_at: now,
        }))
    }

    /// Every stored Report version of the Task, by kind then version.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by show --reports, not yet wired")
    )]
    pub(crate) fn reports(&self, task: TaskId) -> Result<Vec<ReportRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, task, kind, version, path, title, created_at FROM reports \
             WHERE task = ? ORDER BY kind, version",
        )?;
        let rows = stmt
            .query_map([task.get()], report_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Map a `reports` row selected in column order.
fn report_from_row(r: &Row<'_>) -> rusqlite::Result<ReportRow> {
    Ok(ReportRow {
        id: r.get(0)?,
        task: TaskId::new(r.get(1)?),
        kind: r.get(2)?,
        version: r.get(3)?,
        path: r.get(4)?,
        title: r.get(5)?,
        created_at: Ts::from_millis(r.get(6)?),
    })
}

#[cfg(test)]
mod tests;
