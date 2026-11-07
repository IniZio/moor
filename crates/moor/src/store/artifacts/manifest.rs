//! The host-side `artifact-manifest.json` for a Box, in the crabbox artifact
//! manifest shape (schema version 1).

use std::io;

use moor_core::seam::clock::Ts;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::ArtifactRow;
use crate::blobs::{BlobKey, Blobs};

/// Format a timestamp as RFC3339 UTC with second precision.
///
/// A timestamp outside years 0..=9999 (a corrupt row) renders as its raw
/// millisecond count in decimal.
fn rfc3339(ts: Ts) -> String {
    let ms = ts.as_millis();
    OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
}

/// The manifest document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    /// Manifest schema version.
    schema_version: u32,
    /// When the manifest was written, RFC3339 UTC.
    generated_at: String,
    /// Where the files are stored.
    storage: Storage,
    /// One entry per artifact name.
    files: Vec<File>,
}

/// The storage backend block.
#[derive(Serialize)]
struct Storage {
    /// Backend name.
    backend: &'static str,
}

/// One artifact entry.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct File {
    /// Artifact kind.
    kind: String,
    /// Artifact name.
    name: String,
    /// Stored path of the file.
    path: String,
    /// MIME type.
    content_type: String,
    /// Size in bytes.
    size: u64,
    /// Hex SHA-256 of the content.
    sha256: String,
    /// Version number.
    version: i64,
    /// When this version was stored, RFC3339 UTC.
    published_at: String,
}

/// Writes the Box's manifest under `key`, naming each file where [`Blobs::locate`] says it is.
pub(super) fn write_manifest(
    blobs: &dyn Blobs,
    key: &BlobKey,
    rows: &[ArtifactRow],
    generated_at: Ts,
) -> io::Result<()> {
    let manifest = Manifest {
        schema_version: 1,
        generated_at: rfc3339(generated_at),
        storage: Storage {
            backend: blobs.name(),
        },
        files: rows
            .iter()
            .map(|r| File {
                kind: r.kind.clone(),
                name: r.name.clone(),
                path: r.path.clone(),
                content_type: r.content_type.clone(),
                size: r.size,
                sha256: r.sha256.clone(),
                version: r.version,
                published_at: rfc3339(r.created_at),
            })
            .collect(),
    };
    let mut body = serde_json::to_string_pretty(&manifest).map_err(io::Error::other)?;
    body.push('\n');

    blobs.put(key, body.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests;
