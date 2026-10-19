//! `artifacts` and `artifact-read`: serve published files from the store without waking the Box.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moor_core::artifact::valid_name;
use moor_core::request::{ArtifactReadReq, ArtifactsReq, ARTIFACT_CHUNK};
use moor_core::view::{ArtifactChunk, ArtifactView};
use moor_core::{BoxName, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::{artifacts::ArtifactRow, Store};

/// List a Task's Artifacts: the newest version of each name, or every version with `all`.
/// Read-only: no reconcile and no Box lock.
///
/// # Errors
/// [`CommandError`] when the Task is unknown or a store read fails.
pub(crate) fn list(cx: &Cx, req: &ArtifactsReq) -> Result<Vec<ArtifactView>, CommandError> {
    let store = cx.store()?;
    let task = task_id(store, &req.task)?;
    let rows = if req.all {
        store.artifacts(task)?
    } else {
        store.latest_artifacts(task)?
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

/// Read one chunk of an Artifact version, at most [`ARTIFACT_CHUNK`] bytes.
///
/// # Errors
/// [`CommandError`] when the name is invalid, the Artifact is unknown or missing on disk, the
/// offset is past the end, or the stored path is a symlink.
pub(crate) fn read(cx: &Cx, req: &ArtifactReadReq) -> Result<ArtifactChunk, CommandError> {
    if !valid_name(&req.name) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("invalid artifact name `{}`", req.name),
        ));
    }
    let store = cx.store()?;
    let task = task_id(store, &req.task)?;
    let rows = store.artifacts(task)?;
    let row = rows
        .iter()
        .filter(|r| r.name == req.name && req.version.is_none_or(|v| i64::from(v) == r.version))
        .max_by_key(|r| r.version)
        .ok_or_else(|| {
            let which = req.version.map_or_else(String::new, |v| format!(" v{v}"));
            CommandError::new(
                ErrorKind::NotFound,
                format!("no artifact {}{which} for {}", req.name, req.task.as_str()),
            )
        })?;
    let box_dir = cx
        .state_dir()?
        .join("artifacts")
        .join(task.get().to_string());
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
    let (data, eof) = chunk(&file, req).map_err(|e| match e {
        Chunk::Past => CommandError::new(
            ErrorKind::Usage,
            format!("offset {} is past the end of {}", req.offset, row.name),
        ),
        Chunk::Io(e) => io_error(row, &e),
    })?;
    Ok(ArtifactChunk {
        data_b64: STANDARD.encode(data),
        size: row.size,
        sha256: row.sha256.clone(),
        eof,
    })
}

/// Why a chunk could not be read.
enum Chunk {
    /// The offset lies past the end of the file.
    Past,
    /// The file could not be opened or read.
    Io(std::io::Error),
}

/// Read up to the clamped length at the offset; also whether that reaches the end.
fn chunk(file: &Path, req: &ArtifactReadReq) -> Result<(Vec<u8>, bool), Chunk> {
    let mut f = File::open(file).map_err(Chunk::Io)?;
    let size = f.metadata().map_err(Chunk::Io)?.len();
    if req.offset > size {
        return Err(Chunk::Past);
    }
    f.seek(SeekFrom::Start(req.offset)).map_err(Chunk::Io)?;
    let want = req.len.min(ARTIFACT_CHUNK).min(size - req.offset);
    let mut data = Vec::new();
    f.take(want).read_to_end(&mut data).map_err(Chunk::Io)?;
    let end = req.offset + data.len() as u64;
    Ok((data, end >= size))
}

/// Map a file error to a command error that names the Artifact, never its contents.
fn io_error(row: &ArtifactRow, e: &std::io::Error) -> CommandError {
    if e.kind() == std::io::ErrorKind::NotFound {
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

/// Resolve the newest Task with this name.
fn task_id(store: &Store, name: &BoxName) -> Result<moor_core::BoxId, CommandError> {
    store
        .latest_task_by_name(name)?
        .map(|t| t.id)
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no task {}", name.as_str())))
}

#[cfg(test)]
mod tests;
