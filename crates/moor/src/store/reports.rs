//! Report versions kept per box; contents live in the blob store.

use std::io;

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::Reports;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::{Store, StoreError};
use crate::blobs::{BlobError, BlobKey, Blobs};
use crate::store::db::{Db, DbExt as _, OptionalExtension, Row};

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
    /// Where a person finds this version; a reader goes by its key, not this.
    pub(crate) path: String,
    /// Title, if one was recorded.
    pub(crate) title: Option<String>,
    /// When this version was stored.
    pub(crate) created_at: Ts,
}

impl Store {
    /// Stores `content` as the next version of `kind`.
    /// Returns None and writes nothing when the latest stored version holds the same content.
    ///
    /// The blob store is used outside any transaction, as it may be slow or gone and every store
    /// write waits on the one write lock. The Box lock keeps out a second collect.
    pub(crate) fn add_report(
        &self,
        box_id: BoxId,
        kind: &str,
        content: &str,
        blobs: &dyn Blobs,
    ) -> Result<Option<ReportRow>, StoreError> {
        if kind.is_empty() || kind.contains('/') || kind == "." || kind == ".." {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid report kind `{kind}`"),
            )));
        }
        let latest = latest_in(&self.reader()?, box_id, kind)?;
        if let Some(version) = latest {
            match blobs.get(&BlobKey::report(box_id, kind, version)) {
                Ok(old) if old == content.as_bytes() => return Ok(None),
                Ok(_) | Err(BlobError::Missing | BlobError::NotPlain(_)) => {}
                Err(e) => return Err(io::Error::from(e).into()),
            }
        }
        let version = latest.map_or(1, |v| v + 1);
        let key = BlobKey::report(box_id, kind, version);
        blobs
            .put(&key, content.as_bytes())
            .map_err(io::Error::from)?;
        let path = blobs.locate(&key);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let id = insert_in(&tx, box_id, kind, version, &path, now)?;
        tx.commit()?;
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

/// The kind's newest stored version number, if any.
fn latest_in(db: &dyn Db, box_id: BoxId, kind: &str) -> Result<Option<i64>, StoreError> {
    Ok(db
        .row(
            Query::select()
                .column(Reports::Version)
                .from(Reports::Table)
                .and_where(Expr::col(Reports::Box).eq(box_id.get()))
                .and_where(Expr::col(Reports::Kind).eq(kind))
                .order_by(Reports::Version, Order::Desc)
                .limit(1),
            |r| r.get(0),
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
) -> crate::store::db::Result<i64> {
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

/// Map a `reports` row selected in column order.
fn report_from_row(r: &Row<'_>) -> crate::store::db::Result<ReportRow> {
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
