//! `artifacts` and `artifact-read`: serve published files from the store without waking the Box.

use std::io;

use moor_core::artifact::valid_name;
use moor_core::controller::BYTES_MAX;
use moor_core::request::{ArtifactReadReq, ArtifactsReq};
use moor_core::response::ErrorDetail;
use moor_core::view::{ArtifactReadView, ArtifactView};
use moor_core::{BoxTarget, ErrorKind};
use sha2::{Digest, Sha256};

use crate::blobs::{BlobError, BlobKey, Blobs};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::view::rfc3339;
use crate::store::{artifacts::ArtifactRow, BoxRow, Store};

/// List a Box's Artifacts: the newest version of each name, or every version with `all`.
/// Read-only: no reconcile and no Box lock.
///
/// # Errors
/// [`CommandError`] when the Box is unknown or a store read fails.
pub(crate) fn list(cx: &Cx, req: &ArtifactsReq) -> Result<Vec<ArtifactView>, CommandError> {
    let store = cx.store()?;
    let box_id = target_box(store, &req.target)?.id;
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
/// [`CommandError`] when the name is invalid, the Artifact is unknown or missing from the blob
/// store, the offset is past the end, the stored path is unsafe, the blob store cannot be read,
/// or `out` fails.
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
    let found = target_box(store, &req.target)?;
    let (box_id, name) = (found.id, found.name.as_str());
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
    let key = BlobKey::artifact(box_id, row.version, &row.name);
    let whole = (req.offset == 0 && req.len >= row.size).then_some(row.sha256.as_str());
    let (sent, eof) =
        stream(cx.blobs()?, &key, req, whole, out).map_err(|e| e.into_error(row, req.offset))?;
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
    /// The blob could not be read, or the receiver refused a piece.
    Blob(BlobError),
    /// Every byte went out, and they hash to this instead of the recorded sha256.
    Mismatch(String),
}

impl Stream {
    /// The command error for `row` read from `offset`; it never carries file contents.
    fn into_error(self, row: &ArtifactRow, offset: u64) -> CommandError {
        match self {
            Self::Past => CommandError::new(
                ErrorKind::Usage,
                format!("offset {offset} is past the end of {}", row.name),
            ),
            Self::Blob(e) => blob_error(row, &e),
            Self::Mismatch(got) => CommandError::new(
                ErrorKind::Failed,
                format!(
                    "artifact {} v{} does not match its recorded sha256 {}; the stored file \
                     hashes to {got}",
                    row.name, row.version, row.sha256
                ),
            )
            .with_detail(ErrorDetail::ArtifactMismatch),
        }
    }
}

/// Send up to the clamped length from the offset, a piece of at most [`BYTES_MAX`] bytes at a
/// time; returns bytes sent and whether that reaches the end. With `whole`, the bytes sent must
/// hash to it, so a receiver that holds back the last piece until the response never passes on a
/// file that does not match.
fn stream(
    blobs: &dyn Blobs,
    key: &BlobKey,
    req: &ArtifactReadReq,
    whole: Option<&str>,
    out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> Result<(u64, bool), Stream> {
    let size = blobs.size(key).map_err(Stream::Blob)?;
    if req.offset > size {
        return Err(Stream::Past);
    }
    let mut sent = 0_u64;
    let mut digest = Sha256::new();
    let len = req.len.min(size - req.offset);
    blobs
        .read(key, req.offset, len, &mut |chunk| {
            for piece in chunk.chunks(BYTES_MAX) {
                if whole.is_some() {
                    digest.update(piece);
                }
                out(piece)?;
                sent += piece.len() as u64;
            }
            Ok(())
        })
        .map_err(Stream::Blob)?;
    if let Some(want) = whole {
        let got = format!("{:x}", digest.finalize());
        if !got.eq_ignore_ascii_case(want) {
            return Err(Stream::Mismatch(got));
        }
    }
    Ok((sent, req.offset + sent >= size))
}

/// Map a blob error to a command error that names the Artifact, never its contents.
fn blob_error(row: &ArtifactRow, e: &BlobError) -> CommandError {
    match e {
        BlobError::Missing => CommandError::new(
            ErrorKind::NotFound,
            format!(
                "artifact {} v{} is missing from the blob store",
                row.name, row.version
            ),
        )
        .with_detail(ErrorDetail::ArtifactMissing),
        BlobError::NotPlain(_) => CommandError::new(
            ErrorKind::Refused,
            format!("artifact {} is not a plain stored file", row.name),
        ),
        BlobError::Store(why) => CommandError::new(
            ErrorKind::Failed,
            format!("cannot read artifact {}: {why}", row.name),
        ),
        BlobError::Sink(e) => CommandError::new(
            ErrorKind::Failed,
            format!("stopped sending artifact {}: {}", row.name, e.kind()),
        ),
    }
}

/// The Box `target` means, in any state.
fn target_box(store: &Store, target: &BoxTarget) -> Result<BoxRow, CommandError> {
    target::resolve(store, target, Rule::Latest)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no Box {target}")))
}

#[cfg(test)]
mod tests;
