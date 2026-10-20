//! `artifacts` and `artifact-read`: serve published files from the store without waking the Box.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use moor_core::artifact::valid_name;
use moor_core::controller::BYTES_MAX;
use moor_core::request::{ArtifactReadReq, ArtifactsReq};
use moor_core::view::{ArtifactReadView, ArtifactView};
use moor_core::{BoxName, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::{artifacts::ArtifactRow, Store};

/// List a Box's Artifacts: the newest version of each name, or every version with `all`.
/// Read-only: no reconcile and no Box lock.
///
/// # Errors
/// [`CommandError`] when the Box is unknown or a store read fails.
pub(crate) fn list(cx: &Cx, req: &ArtifactsReq) -> Result<Vec<ArtifactView>, CommandError> {
    let store = cx.store()?;
    let box_id = named_box(store, &req.box_name)?;
    let rows = if req.all {
        store.artifacts(box_id)?
    } else {
        store.latest_artifacts(box_id)?
    };
    Ok(rows.iter().map(artifact_view).collect())
}

/// Map a stored row to its wire view.
pub(crate) fn artifact_view(row: &ArtifactRow) -> ArtifactView {
    ArtifactView {
        name: row.name.clone(),
        version: u32::try_from(row.version).unwrap_or(0),
        kind: row.kind.clone(),
        content_type: row.content_type.clone(),
        size: row.size,
        sha256: row.sha256.clone(),
        created_at: rfc3339(row.created_at),
    }
}

/// Stream a byte range of an Artifact version to `out`, one piece of at most [`BYTES_MAX`]
/// bytes at a time, reading the next piece only after `out` took the last one.
///
/// # Errors
/// [`CommandError`] when the name is invalid, the Artifact is unknown or missing on disk, the
/// offset is past the end, the stored path is a symlink, a read fails, or `out` fails.
pub(crate) fn read(
    cx: &Cx,
    req: &ArtifactReadReq,
    out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> Result<ArtifactReadView, CommandError> {
    if !valid_name(&req.name) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("invalid artifact name `{}`", req.name),
        ));
    }
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let box_id = named_box(store, &req.box_name)?;
    let rows = store.artifacts(box_id)?;
    let row = rows
        .iter()
        .filter(|r| r.name == req.name && req.version.is_none_or(|v| i64::from(v) == r.version))
        .max_by_key(|r| r.version)
        .ok_or_else(|| {
            let which = req.version.map_or_else(String::new, |v| format!(" v{v}"));
            CommandError::new(
                ErrorKind::NotFound,
                format!("no artifact {}{which} for {name}", req.name),
            )
        })?;
    let box_dir = cx
        .state_dir()?
        .join("artifacts")
        .join(box_id.get().to_string());
    let version_dir = box_dir.join(format!("v{}", row.version));
    let file = version_dir.join(&row.name);
    let refused = || {
        CommandError::new(
            ErrorKind::Refused,
            format!("artifact {} is not a plain stored file", row.name),
        )
    };
    for dir in [&box_dir, &version_dir] {
        match fs::symlink_metadata(dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(refused()),
            Err(e) => return Err(io_error(row, &e)),
        }
    }
    match fs::symlink_metadata(&file) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(refused()),
        Err(e) => return Err(io_error(row, &e)),
    }
    let (sent, eof) = stream(&file, req, out).map_err(|e| e.into_error(row, req.offset))?;
    Ok(ArtifactReadView {
        offset: req.offset,
        sent,
        size: row.size,
        sha256: row.sha256.clone(),
        eof,
    })
}

/// Why a range could not be streamed.
enum Stream {
    /// The offset lies past the end of the file.
    Past,
    /// The file could not be opened or read.
    Io(io::Error),
    /// The receiver refused a piece.
    Out(io::Error),
}

impl Stream {
    /// The command error for `row` read from `offset`; it never carries file contents.
    fn into_error(self, row: &ArtifactRow, offset: u64) -> CommandError {
        match self {
            Self::Past => CommandError::new(
                ErrorKind::Usage,
                format!("offset {offset} is past the end of {}", row.name),
            ),
            Self::Io(e) => io_error(row, &e),
            Self::Out(e) => CommandError::new(
                ErrorKind::Failed,
                format!("stopped sending artifact {}: {}", row.name, e.kind()),
            ),
        }
    }
}

/// Send up to the clamped length from the offset; returns bytes sent and whether that reaches
/// the end.
fn stream(
    file: &Path,
    req: &ArtifactReadReq,
    out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> Result<(u64, bool), Stream> {
    let mut f = File::open(file).map_err(Stream::Io)?;
    let size = f.metadata().map_err(Stream::Io)?.len();
    if req.offset > size {
        return Err(Stream::Past);
    }
    f.seek(SeekFrom::Start(req.offset)).map_err(Stream::Io)?;
    let mut f = f.take(req.len.min(size - req.offset));
    let mut buf = vec![0; BYTES_MAX];
    let mut sent = 0_u64;
    loop {
        let n = match f.read(&mut buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            n => n.map_err(Stream::Io)?,
        };
        let Some(piece) = buf.get(..n).filter(|p| !p.is_empty()) else {
            return Ok((sent, req.offset + sent >= size));
        };
        out(piece).map_err(Stream::Out)?;
        sent += n as u64;
    }
}

/// Map a file error to a command error that names the Artifact, never its contents.
fn io_error(row: &ArtifactRow, e: &io::Error) -> CommandError {
    if e.kind() == io::ErrorKind::NotFound {
        return CommandError::new(
            ErrorKind::NotFound,
            format!("artifact {} v{} is missing on disk", row.name, row.version),
        );
    }
    CommandError::new(
        ErrorKind::Failed,
        format!("cannot read artifact {}: {}", row.name, e.kind()),
    )
}

/// Resolve the newest Box with this name.
fn named_box(store: &Store, name: &BoxName) -> Result<moor_core::BoxId, CommandError> {
    store
        .latest_box_by_name(name)?
        .map(|t| t.id)
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no Box {}", name.as_str())))
}

#[cfg(test)]
mod tests;
