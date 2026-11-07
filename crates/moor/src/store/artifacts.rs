//! Artifact versions kept per Box; contents live in the blob store, rows hold the digest.

use std::fs;
use std::io;
use std::path::Path;

use moor_core::artifact::{content_type, kind_of, valid_name};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::Artifacts;
use sea_query::{Expr, ExprTrait as _, Order, Query, SelectStatement};

use super::select::newest_session;
use super::{Store, StoreError};
use crate::blobs::{BlobError, BlobKey, Blobs};
use crate::store::db::{self, Db, DbExt as _, OptionalExtension, Row, Tx};

/// Columns of an `artifacts` row, in the order `artifact_from_row` reads them.
const COLUMNS: [Artifacts; 11] = [
    Artifacts::Id,
    Artifacts::Box,
    Artifacts::Session,
    Artifacts::Name,
    Artifacts::Version,
    Artifacts::Kind,
    Artifacts::ContentType,
    Artifacts::Size,
    Artifacts::Sha256,
    Artifacts::Path,
    Artifacts::CreatedAt,
];

/// Every artifact row, with the columns `artifact_from_row` reads.
fn artifact_select() -> SelectStatement {
    Query::select()
        .columns(COLUMNS)
        .from(Artifacts::Table)
        .take()
}

/// One stored version of a file published by a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning Box.
    pub(crate) box_id: BoxId,
    /// Session that was newest when this version was stored.
    pub(crate) session: Option<i64>,
    /// File name; versions are counted per name.
    pub(crate) name: String,
    /// Version number, starting at 1.
    pub(crate) version: i64,
    /// Artifact kind derived from the name.
    pub(crate) kind: String,
    /// Content type derived from the name.
    pub(crate) content_type: String,
    /// Size in bytes.
    pub(crate) size: u64,
    /// Lowercase hex SHA-256 of the contents.
    pub(crate) sha256: String,
    /// Where a person finds this version; a reader goes by its key, not this.
    pub(crate) path: String,
    /// When this version was stored.
    pub(crate) created_at: Ts,
}

impl Store {
    /// Stores `bytes` as the next version of `name`.
    /// Returns None and writes nothing when the latest stored version has the same digest.
    /// This is the in-memory form tests use; the collect path stages files and moves them.
    #[cfg(test)]
    pub(crate) fn add_artifact(
        &self,
        box_id: BoxId,
        name: &str,
        bytes: &[u8],
        blobs: &dyn Blobs,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        use sha2::{Digest, Sha256};

        let contents = Contents {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
        };
        self.add_version(box_id, name, blobs, contents, |key| blobs.put(key, bytes))
    }

    /// Stores the regular file at `staged` as the next version of `name`, which takes it;
    /// `sha256` is its digest, already checked by the caller.
    /// Returns None and leaves `staged` in place when the latest stored version has the same digest.
    pub(crate) fn add_artifact_staged(
        &self,
        box_id: BoxId,
        name: &str,
        staged: &Path,
        sha256: &str,
        blobs: &dyn Blobs,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        let meta = fs::symlink_metadata(staged)?;
        if !meta.file_type().is_file() {
            return Err(StoreError::Io(io::Error::other(format!(
                "{} is not a regular file",
                staged.display()
            ))));
        }
        let contents = Contents {
            sha256: sha256.to_owned(),
            size: meta.len(),
        };
        self.add_version(box_id, name, blobs, contents, |key| {
            blobs.put_file(key, staged)
        })
    }

    /// Records the next version of `name` unless the digest matches the latest one, calling
    /// `place` to store the contents under the version's key.
    ///
    /// `place` runs outside any transaction, as the blob store may be slow or gone and every
    /// store write waits on the one write lock. The Box lock keeps out a second collect, and a
    /// blob whose row never committed is replaced when that version is tried again.
    fn add_version(
        &self,
        box_id: BoxId,
        name: &str,
        blobs: &dyn Blobs,
        contents: Contents,
        place: impl FnOnce(&BlobKey) -> Result<(), BlobError>,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        let Contents { sha256, size } = contents;
        if !valid_name(name) {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid artifact name `{name}`"),
            )));
        }
        let latest: Option<(i64, String)> = self
            .reader()?
            .row(
                Query::select()
                    .columns([Artifacts::Version, Artifacts::Sha256])
                    .from(Artifacts::Table)
                    .and_where(Expr::col(Artifacts::Box).eq(box_id.get()))
                    .and_where(Expr::col(Artifacts::Name).eq(name))
                    .order_by(Artifacts::Version, Order::Desc)
                    .limit(1),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if latest.as_ref().is_some_and(|(_, old)| *old == sha256) {
            return Ok(None);
        }
        let version = latest.map_or(1, |(v, _)| v + 1);
        let key = BlobKey::artifact(box_id, version, name);
        place(&key).map_err(io::Error::from)?;
        let now = self.now();
        let fresh = Fresh {
            box_id,
            name,
            version,
            size,
            sha256,
            path: blobs.locate(&key),
            now,
        };
        let mut conn = self.writer();
        let row = record(conn.immediate()?, &fresh)?;
        drop(conn);
        let rows = self.latest_artifacts(box_id)?;
        manifest::write_manifest(blobs, &BlobKey::artifact_manifest(box_id), &rows, now)?;
        Ok(Some(row))
    }

    /// Every stored Artifact version of the Box, by name then version.
    pub(crate) fn artifacts(&self, box_id: BoxId) -> Result<Vec<ArtifactRow>, StoreError> {
        let conn = self.reader()?;
        let q = artifact_select()
            .and_where(Expr::col(Artifacts::Box).eq(box_id.get()))
            .order_by(Artifacts::Name, Order::Asc)
            .order_by(Artifacts::Version, Order::Asc)
            .take();
        Ok(conn.rows(q, artifact_from_row)?.collect::<Result<_, _>>()?)
    }

    /// The newest stored version of each Artifact name of the Box, by name.
    pub(crate) fn latest_artifacts(&self, box_id: BoxId) -> Result<Vec<ArtifactRow>, StoreError> {
        let conn = self.reader()?;
        latest_in(&conn, box_id)
    }
}

/// Digest and size of the bytes of a version.
struct Contents {
    /// Lowercase hex digest.
    sha256: String,
    /// Size in bytes.
    size: u64,
}

/// A version about to be recorded.
struct Fresh<'a> {
    /// Owning Box.
    box_id: BoxId,
    /// File name.
    name: &'a str,
    /// Version number.
    version: i64,
    /// Size in bytes.
    size: u64,
    /// Lowercase hex digest.
    sha256: String,
    /// Where a person finds it.
    path: String,
    /// Time of the write.
    now: Ts,
}

/// Inserts the row and event, and commits.
fn record(tx: Tx<'_>, f: &Fresh<'_>) -> Result<ArtifactRow, StoreError> {
    let kind = kind_of(f.name).as_str().to_owned();
    let content_type = content_type(f.name).to_owned();
    let size = i64::try_from(f.size)
        .map_err(|_| StoreError::Corrupt("artifact size exceeds the column range".to_owned()))?;
    let id: i64 = tx.row(
        Query::insert()
            .into_table(Artifacts::Table)
            .columns(COLUMNS.iter().skip(1).copied())
            .values_panic([
                f.box_id.get().into(),
                db::sub(newest_session(f.box_id)),
                f.name.into(),
                f.version.into(),
                kind.clone().into(),
                content_type.clone().into(),
                size.into(),
                f.sha256.clone().into(),
                f.path.clone().into(),
                f.now.as_millis().into(),
            ])
            .returning_col(Artifacts::Id),
        |r| r.get(0),
    )?;
    let event = Event::ArtifactPublished {
        name: f.name.to_owned(),
        version: f.version,
        artifact_kind: kind,
        content_type,
        size: f.size,
        sha256: f.sha256.clone(),
    };
    super::apply::insert_event(&tx, f.box_id, &event, f.now)?;
    let row = tx.row(
        artifact_select().and_where(Expr::col(Artifacts::Id).eq(id)),
        artifact_from_row,
    )?;
    tx.commit()?;
    Ok(row)
}

/// Newest version per name for the Box, read on `conn`.
fn latest_in(conn: &dyn Db, box_id: BoxId) -> Result<Vec<ArtifactRow>, StoreError> {
    let newest = Query::select()
        .expr(Expr::col(("newer", Artifacts::Version)).max())
        .from_as(Artifacts::Table, "newer")
        .and_where(Expr::col(("newer", Artifacts::Box)).equals((Artifacts::Table, Artifacts::Box)))
        .and_where(
            Expr::col(("newer", Artifacts::Name)).equals((Artifacts::Table, Artifacts::Name)),
        )
        .take();
    let q = artifact_select()
        .and_where(Expr::col(Artifacts::Box).eq(box_id.get()))
        .and_where(Expr::col(Artifacts::Version).eq(db::sub(newest)))
        .order_by(Artifacts::Name, Order::Asc)
        .take();
    Ok(conn.rows(q, artifact_from_row)?.collect::<Result<_, _>>()?)
}

/// Map an `artifacts` row selected in `COLUMNS` order.
fn artifact_from_row(r: &Row<'_>) -> db::Result<ArtifactRow> {
    let size: i64 = r.get(7)?;
    Ok(ArtifactRow {
        id: r.get(0)?,
        box_id: BoxId::new(r.get(1)?),
        session: r.get(2)?,
        name: r.get(3)?,
        version: r.get(4)?,
        kind: r.get(5)?,
        content_type: r.get(6)?,
        size: u64::try_from(size).map_err(|_| db::Error::OutOfRange {
            index: 7,
            value: size,
        })?,
        sha256: r.get(8)?,
        path: r.get(9)?,
        created_at: Ts::from_millis(r.get(10)?),
    })
}

mod due;
mod manifest;
#[cfg(test)]
mod tests;
