//! Artifact versions kept per Box; contents live as files, rows hold the path.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use moor_core::artifact::{content_type, kind_of, valid_name};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, OptionalExtension, Row, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::{Store, StoreError};

/// DDL for the artifacts table; idempotent so existing files gain it on open.
pub(super) const ARTIFACTS_SQL: &str = include_str!("artifacts.sql");

/// Columns of an `artifacts` row, in the order `artifact_from_row` reads them.
const COLUMNS: &str =
    "id, box, session, name, version, kind, content_type, size, sha256, path, created_at";

/// One stored version of a file published by a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning Box.
    pub(crate) task: BoxId,
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by later callers, not yet wired")
    )]
    pub(crate) fn add_artifact(
        &self,
        task: BoxId,
        name: &str,
        bytes: &[u8],
        dir: &Path,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        if !valid_name(name) {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid artifact name `{name}`"),
            )));
        }
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest: Option<(i64, String)> = tx
            .query_row(
                "SELECT version, sha256 FROM artifacts WHERE box = ? AND name = ? \
                 ORDER BY version DESC LIMIT 1",
                params![task.get(), name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if latest.as_ref().is_some_and(|(_, old)| *old == sha256) {
            return Ok(None);
        }
        let version = latest.map_or(1, |(v, _)| v + 1);
        let box_dir = dir.join(task.get().to_string());
        let version_dir = box_dir.join(format!("v{version}"));
        let file = version_dir.join(name);
        refuse_symlink(dir, &file)?;
        clear_stale(&file)?;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&version_dir)?;
        write_private(&file, bytes)?;
        let fresh = Fresh {
            task,
            name,
            version,
            size: bytes.len() as u64,
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by later callers, not yet wired")
    )]
    pub(crate) fn artifacts(&self, task: BoxId) -> Result<Vec<ArtifactRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM artifacts WHERE box = ? ORDER BY name, version"
        ))?;
        let rows = stmt
            .query_map([task.get()], artifact_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The newest stored version of each Artifact name of the Box, by name.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by later callers, not yet wired")
    )]
    pub(crate) fn latest_artifacts(&self, task: BoxId) -> Result<Vec<ArtifactRow>, StoreError> {
        let conn = self.reader()?;
        latest_in(&conn, task)
    }
}

/// A version about to be recorded.
struct Fresh<'a> {
    /// Owning Box.
    task: BoxId,
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
fn record(tx: Transaction<'_>, f: &Fresh<'_>, box_dir: &Path) -> Result<ArtifactRow, StoreError> {
    let kind = kind_of(f.name).as_str().to_owned();
    let content_type = content_type(f.name).to_owned();
    let size = i64::try_from(f.size)
        .map_err(|_| StoreError::Corrupt("artifact size exceeds the column range".to_owned()))?;
    tx.execute(
        "INSERT INTO artifacts (box, session, name, version, kind, content_type, size, sha256, \
         path, created_at) VALUES (?1, \
         (SELECT id FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1), \
         ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            f.task.get(),
            f.name,
            f.version,
            kind,
            content_type,
            size,
            f.sha256,
            f.path,
            f.now.as_millis()
        ],
    )?;
    let id = tx.last_insert_rowid();
    let event = Event::ArtifactPublished {
        name: f.name.to_owned(),
        version: f.version,
        artifact_kind: kind,
        content_type,
        size: f.size,
        sha256: f.sha256.clone(),
    };
    super::apply::insert_event(&tx, f.task, &event, f.now)?;
    let rows = latest_in(&tx, f.task)?;
    manifest::write_manifest(box_dir, &rows, f.now)?;
    let row = tx.query_row(
        &format!("SELECT {COLUMNS} FROM artifacts WHERE id = ?"),
        [id],
        artifact_from_row,
    )?;
    tx.commit()?;
    Ok(row)
}

/// Newest version per name for the Box, read on `conn`.
fn latest_in(conn: &rusqlite::Connection, task: BoxId) -> Result<Vec<ArtifactRow>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM artifacts a WHERE box = ?1 AND version = \
         (SELECT MAX(version) FROM artifacts WHERE box = a.box AND name = a.name) ORDER BY name"
    ))?;
    let rows = stmt
        .query_map([task.get()], artifact_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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

/// Creates `file` new, readable by the owner only, and writes `bytes` to it.
fn write_private(file: &Path, bytes: &[u8]) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)?
        .write_all(bytes)?;
    fs::set_permissions(file, Permissions::from_mode(0o600))
}

/// Map an `artifacts` row selected in `COLUMNS` order.
fn artifact_from_row(r: &Row<'_>) -> rusqlite::Result<ArtifactRow> {
    let size: i64 = r.get(7)?;
    Ok(ArtifactRow {
        id: r.get(0)?,
        task: BoxId::new(r.get(1)?),
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

mod manifest;
#[cfg(test)]
mod tests;
