//! Report versions kept per box; contents live as files, rows hold the path.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::Reports;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::{Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension, Row, TransactionBehavior};

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
        let latest = latest_in(&tx, box_id, kind)?;
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
        let stored = insert_in(&tx, box_id, kind, version, &path, now)
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
        let q = Query::select()
            .columns([
                Reports::Id,
                Reports::Box,
                Reports::Kind,
                Reports::Version,
                Reports::Path,
                Reports::Title,
                Reports::CreatedAt,
            ])
            .from(Reports::Table)
            .and_where(Expr::col(Reports::Box).eq(box_id.get()))
            .order_by(Reports::Kind, Order::Asc)
            .order_by(Reports::Version, Order::Asc)
            .take();
        Ok(conn.rows(q, report_from_row)?.collect::<Result<_, _>>()?)
    }
}

/// The kind's newest stored version number and file path, if any.
fn latest_in(db: &dyn Db, box_id: BoxId, kind: &str) -> Result<Option<(i64, String)>, StoreError> {
    Ok(db
        .row(
            Query::select()
                .columns([Reports::Version, Reports::Path])
                .from(Reports::Table)
                .and_where(Expr::col(Reports::Box).eq(box_id.get()))
                .and_where(Expr::col(Reports::Kind).eq(kind))
                .order_by(Reports::Version, Order::Desc)
                .limit(1),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Inserts version `version` of `kind` untitled; its row id.
fn insert_in(
    db: &dyn Db,
    box_id: BoxId,
    kind: &str,
    version: i64,
    path: &str,
    now: Ts,
) -> rusqlite::Result<i64> {
    db.row(
        Query::insert()
            .into_table(Reports::Table)
            .columns([
                Reports::Box,
                Reports::Kind,
                Reports::Version,
                Reports::Path,
                Reports::Title,
                Reports::CreatedAt,
            ])
            .values_panic([
                box_id.get().into(),
                kind.into(),
                version.into(),
                path.into(),
                None::<String>.into(),
                now.as_millis().into(),
            ])
            .returning_col(Reports::Id),
        |r| r.get(0),
    )
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
