//! The timeline lines of a saved archive: read from Claude's session files once, as the archive
//! is saved, and kept beside it, so a page never opens the archive and a Box that sleeps or is
//! closed still shows them.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use flate2::read::GzDecoder;
use moor_core::progress::transcript::{Line, Reader, STORED_MAX};
use moor_core::{BoxId, Warning};

use crate::blobs::{BlobError, BlobKey, Blobs};
use crate::cx::Cx;
use crate::store::Store;

/// The lines of Claude's session files in the archive at `archive`: every `*.jsonl` at its top.
/// A subagent's own files are left out; its call shows as one line of the session.
pub(crate) fn read(archive: &Path) -> io::Result<Vec<Line>> {
    let gz = GzDecoder::new(BufReader::new(File::open(archive)?));
    let mut tar = tar::Archive::new(gz);
    let mut reader = Reader::default();
    for entry in tar.entries()? {
        let entry = entry?;
        if !is_session(&entry.path()?) {
            continue;
        }
        records(entry, &mut reader)?;
    }
    Ok(reader.finish())
}

/// Whether `path` in the archive is a session file at its top.
fn is_session(path: &Path) -> bool {
    let path = path.strip_prefix(".").unwrap_or(path);
    let mut parts = path.components();
    let (Some(name), None) = (parts.next(), parts.next()) else {
        return false;
    };
    let name = name.as_os_str().to_string_lossy();
    name.ends_with(".jsonl") && !name.starts_with("agent-")
}

/// Passes each record of `file` to `reader`; a record that is not UTF-8 is skipped.
fn records(file: impl Read, reader: &mut Reader) -> io::Result<()> {
    for record in BufReader::new(file).split(b'\n') {
        if let Ok(text) = std::str::from_utf8(&record?) {
            reader.read(text);
        }
    }
    Ok(())
}

/// Keeps `lines` beside the archive at `archive`. A failure is a warning only: the archive
/// itself is saved, and the timeline keeps the one-line steps.
pub(crate) fn keep(cx: &Cx, blobs: &dyn Blobs, archive: &BlobKey, lines: io::Result<Vec<Line>>) {
    let kept = lines
        .map_err(|e| e.to_string())
        .and_then(|lines| serde_json::to_vec(&lines).map_err(|e| e.to_string()))
        .and_then(|bytes| {
            blobs
                .put(&BlobKey::lines_of(archive.as_str()), &bytes)
                .map_err(|e| e.to_string())
        });
    if let Err(why) = kept {
        cx.warn(Warning {
            code: "transcript_lines_failed".to_owned(),
            message: format!("{}: {why}", archive.as_str()),
        });
    }
}

/// The lines kept beside Box `id`'s newest archive, with when it was taken in milliseconds;
/// none when it has no archive or its lines were never kept.
pub(crate) fn of(cx: &Cx, store: &Store, id: BoxId) -> Option<(Vec<Line>, i64)> {
    let archive = store.newest_archive(id).ok()??;
    let blobs = cx.blobs().ok()?;
    let key = BlobKey::lines_of(&archive.key);
    let read = blobs
        .size(&key)
        .and_then(|size| {
            if size > STORED_MAX {
                return Err(BlobError::Store(format!("{size} bytes")));
            }
            blobs.get(&key)
        })
        .and_then(|bytes| {
            serde_json::from_slice(&bytes).map_err(|e| BlobError::Store(e.to_string()))
        });
    match read {
        Ok(lines) => Some((lines, archive.at.as_millis())),
        Err(BlobError::Missing) => None,
        Err(e) => {
            cx.warn(Warning {
                code: "transcript_lines_unread".to_owned(),
                message: format!("{}: {e}", key.as_str()),
            });
            None
        }
    }
}

#[cfg(test)]
mod tests;
