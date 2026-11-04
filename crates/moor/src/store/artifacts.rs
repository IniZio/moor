//! Artifact versions kept per Box; contents live as files, rows hold the path.

use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use moor_core::artifact::{content_type, kind_of, valid_name};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::Artifacts;
use sea_query::{Expr, ExprTrait as _, Order, Query, SelectStatement};

use super::select::newest_session;
use super::{Store, StoreError};
use crate::store::db::{self, Db, DbExt as _, OptionalExtension, Row, TransactionBehavior, Tx};

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
    /// Full path of the file holding this version.
    pub(crate) path: String,
    /// When this version was stored.
    pub(crate) created_at: Ts,
}

impl Store {
    /// Stores `bytes` as the next version of `name`, under `dir/<box id>/v<version>/<name>`.
    /// Returns None and writes nothing when the latest stored version has the same digest.
    /// This is the in-memory form tests use; the collect path stages files and moves them.
    #[cfg(test)]
    pub(crate) fn add_artifact(
        &self,
        box_id: BoxId,
        name: &str,
        bytes: &[u8],
        dir: &Path,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        use sha2::{Digest, Sha256};

        let contents = Contents {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
        };
        self.add_version(box_id, name, dir, contents, |file| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(file)?
                .write_all(bytes)?;
            fs::set_permissions(file, Permissions::from_mode(0o600))
        })
    }

    /// Stores the regular file at `staged` as the next version of `name` by moving it under
    /// `dir/<box id>/v<version>/<name>`; `sha256` is its digest, already checked by the caller.
    /// Returns None and leaves `staged` in place when the latest stored version has the same digest.
    pub(crate) fn add_artifact_staged(
        &self,
        box_id: BoxId,
        name: &str,
        staged: &Path,
        sha256: &str,
        dir: &Path,
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
        self.add_version(box_id, name, dir, contents, |file| {
            move_private(staged, file)
        })
    }

    /// Records the next version of `name` unless the digest matches the latest one, calling
    /// `place` to put the contents at the version's path; `place` runs inside the transaction.
    fn add_version(
        &self,
        box_id: BoxId,
        name: &str,
        dir: &Path,
        contents: Contents,
        place: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        let Contents { sha256, size } = contents;
        if !valid_name(name) {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid artifact name `{name}`"),
            )));
        }
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest: Option<(i64, String)> = tx
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
        let box_dir = dir.join(box_id.get().to_string());
        let version_dir = box_dir.join(format!("v{version}"));
        let file = version_dir.join(name);
        refuse_symlink(dir, &file)?;
        clear_stale(&file)?;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&version_dir)?;
        place(&file)?;
        let fresh = Fresh {
            box_id,
            name,
            version,
            size,
            sha256,
            path: file.to_string_lossy().into_owned(),
            now,
        };
        match record(tx, &fresh, &box_dir) {
            Ok(row) => {
                drop(conn);
                Ok(Some(row))
            }
            Err(e) => {
                let _ = fs::remove_file(&file);
                Err(e)
            }
        }
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
    /// Full path of the written file.
    path: String,
    /// Time of the write.
    now: Ts,
}

/// Inserts the row and event, rewrites the manifest, and commits.
fn record(tx: Tx<'_>, f: &Fresh<'_>, box_dir: &Path) -> Result<ArtifactRow, StoreError> {
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
    let rows = latest_in(&tx, f.box_id)?;
    manifest::write_manifest(box_dir, &rows, f.now)?;
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

/// Fails when `root` or any existing component down to `to` is a symlink.
fn refuse_symlink(root: &Path, to: &Path) -> io::Result<()> {
    match symlink_on_path(root, to) {
        Ok(None) => Ok(()),
        Ok(Some(link)) => Err(io::Error::other(format!(
            "refusing to write through symlink {}",
            link.display()
        ))),
        Err(msg) => Err(io::Error::other(msg)),
    }
}

/// The first symlink among `root` and every existing path component from there down to `to`.
fn symlink_on_path(root: &Path, to: &Path) -> Result<Option<PathBuf>, String> {
    let rest = to.strip_prefix(root).unwrap_or(to);
    let mut cur = root.to_path_buf();
    let steps = std::iter::once(None).chain(rest.components().map(Some));
    for step in steps {
        if let Some(c) = step {
            cur.push(c);
        }
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Ok(Some(cur)),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot inspect {}: {e}", cur.display())),
        }
    }
    Ok(None)
}

/// Removes a regular file left at `file` by a write whose row never committed.
///
/// Anything else already at `file`, such as a directory, is an error.
fn clear_stale(file: &Path) -> io::Result<()> {
    match fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_file() => fs::remove_file(file),
        Ok(_) => Err(io::Error::other(format!(
            "{} exists and is not a regular file",
            file.display()
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Moves the regular file `from` to the new path `to`, readable by the owner only.
///
/// Falls back to copy and remove when the two paths are on different filesystems.
fn move_private(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            fs::copy(from, to)?;
            fs::remove_file(from)?;
        }
        Err(e) => return Err(e),
    }
    fs::set_permissions(to, Permissions::from_mode(0o600))
}

/// Map an `artifacts` row selected in `COLUMNS` order.
fn artifact_from_row(r: &Row<'_>) -> rusqlite::Result<ArtifactRow> {
    let size: i64 = r.get(7)?;
    Ok(ArtifactRow {
        id: r.get(0)?,
        box_id: BoxId::new(r.get(1)?),
        session: r.get(2)?,
        name: r.get(3)?,
        version: r.get(4)?,
        kind: r.get(5)?,
        content_type: r.get(6)?,
        size: u64::try_from(size).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(7, size))?,
        sha256: r.get(8)?,
        path: r.get(9)?,
        created_at: Ts::from_millis(r.get(10)?),
    })
}

mod due;
mod manifest;
#[cfg(test)]
mod tests;
