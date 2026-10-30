//! Report versions kept per box; contents live as files, rows hold the path.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};

use super::{Store, StoreError};

/// One stored version of a Box's Report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning box.
    pub(crate) box_id: BoxId,
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
    /// Stores `content` as the next version of `kind`, under `dir/<box id>/<kind>.v<version>.md`.
    /// Returns None and writes nothing when the latest stored version holds the same content.
    pub(crate) fn add_report(
        &self,
        box_id: BoxId,
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
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest: Option<(i64, String)> = tx
            .query_row(
                "SELECT version, path FROM reports WHERE box = ? AND kind = ? \
                 ORDER BY version DESC LIMIT 1",
                params![box_id.get(), kind],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((_, path)) = &latest {
            if fs::read(path).is_ok_and(|old| old == content.as_bytes()) {
                return Ok(None);
            }
        }
        let version = latest.map_or(1, |(v, _)| v + 1);
        let box_dir = dir.join(box_id.get().to_string());
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&box_dir)?;
        let file = box_dir.join(format!("{kind}.v{version}.md"));
        let path = file.to_string_lossy().into_owned();
        write_private(&file, content)?;
        let stored = tx
            .execute(
                "INSERT INTO reports (box, kind, version, path, title, created_at) \
                 VALUES (?, ?, ?, ?, NULL, ?)",
                params![box_id.get(), kind, version, path, now.as_millis()],
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
            box_id,
            kind: kind.to_owned(),
            version,
            path,
            title: None,
            created_at: now,
        }))
    }

    /// Every stored Report version of the Box, by kind then version.
    pub(crate) fn reports(&self, box_id: BoxId) -> Result<Vec<ReportRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, box, kind, version, path, title, created_at FROM reports \
             WHERE box = ? ORDER BY kind, version",
        )?;
        let rows = stmt
            .query_map([box_id.get()], report_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Writes `content` to `file`, readable by the owner only.
fn write_private(file: &Path, content: &str) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(file)?
        .write_all(content.as_bytes())?;
    fs::set_permissions(file, Permissions::from_mode(0o600))
}

/// Map a `reports` row selected in column order.
fn report_from_row(r: &Row<'_>) -> rusqlite::Result<ReportRow> {
    Ok(ReportRow {
        id: r.get(0)?,
        box_id: BoxId::new(r.get(1)?),
        kind: r.get(2)?,
        version: r.get(3)?,
        path: r.get(4)?,
        title: r.get(5)?,
        created_at: Ts::from_millis(r.get(6)?),
    })
}

#[cfg(test)]
mod tests;
